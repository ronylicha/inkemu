# SP1b-3b — Flash writes and the device's flash file: a firmware keeps what it stores, across restarts and across runs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **As executed.** This plan is the record of what was intended; it is not rewritten. The code of the fourteen tasks is the plan's own: it was imported, one commit per task, from the clone in which the code blocks below were compiled, tested and mutated, then reviewed by six independent reviewers, one per domain, and corrected. The following changed after the tasks:
>
> - **A data phase SPI1 cannot carry is cut and rounded** (Tasks 2 and 4) → a `USR` transfer whose data phase is longer than 512 bits, or is not whole bytes, is refused by name before anything is sent; the hard-wired read is judged the same way.
> - **Seven commands run in any frame** (Task 3) → every command the chip runs has one frame (`FlashFrame::of`) and is refused in any other; a write is judged on its frame, then its latch, then its range and protection.
> - **A flash refusal is listed once per value written** → the keys are bounded, 1,300 entries at most; several command bits in one write of `CMD` are one line for the run.
> - **An erase reads and fills its whole unit** (Task 3) → the chip keeps the exact set of its sectors that are not blank: an erase costs what it erases, and the image is examined once when the chip is built. Memory is as before: a chip erase does not give the image back, and never did (it was emptied, its allocation kept); the bound is the size of the chip.
> - **A chip erase makes every page read stale, and decoded code run again is no read** (Tasks 5 and 6) → a chip erase makes stale the pages it changed only (the controller asks the chip what its pages hold before that command and no other); an invalidation that takes a page out of the cache moves the generation, so flash code run again counts as read again; an access across two stale pages reports both; what each reset does to the stale state is stated and pinned.
> - **ROM, bootloader and ESP-IDF "never" raise the stale-read diagnostic** (Task 6, ADR 0002) → no path that was read or measured raises it, and ESP-IDF's ranged invalidation is held by unit tests only; the ADR lists five cases where the diagnostic is silent and ten where it speaks.
> - **The flash file takes the default mode and is judged when the run ends** (Tasks 8 to 10) → created `0600`, a replaced file keeps its mode, a file the user cannot write is refused at start; `--flash` and `--flash-file` that name the same file are refused; what would keep the file from being saved is refused before the ROM is loaded; the lock file is never followed nor waited on, and the flash file is read through the same open; the temporary file has one fixed name, `.<name>.tmp`, and what sits there is removed, and said on standard error, by the next run that is accepted, never by a run that is refused; a file removed during the run is written again.
> - **`1 of its 4 KiB sectors were modified`** → `was modified`; the refusals of the flash say the frame a command takes, what `W0..W15` keep and a remedy for each cause; the help of `--flash-file` says who can read the file.
> - **The trace hash of the `hello_world` life is a number the documents quote** → a test pins it, with its device time.
> - **Ignored tests: 18, 16, 12 and 3** (Tasks 12 and 13) → 18, 24, 13 and 4; four mutations that survive Espressif's code → three, the erase being proven on the unmodified bootloader.
> - **`--flash` is refused when it names the flash file** (Task 10) → also when it names the temporary file or the lock file of the flash file, by any route; **a second Ctrl-C always exits 130** → once the summary has begun, Ctrl-C ends nothing and the summary and the save complete; every removal at the temporary name is said, without presuming what was there; **the chip is asked what its pages hold before every chip erase** → only before one it would run. ADR 0004 is renamed `0004-entrees-d-une-execution-et-fichier-de-flash.md`.
> - **`ASSUMPTION:` lines**: 77 at the base, 86 at the end of the branch; eleven added, two removed. The last two added mark the chip's latch and status across every reset of the ESP32-C3, and the opcode `FLASH_CE` sends. `cargo test --workspace`: 1,005 passed, 60 ignored.
>
> The code blocks of the tasks are not the code as it stands on the branch, and the test counts, messages and document texts they state are not those of the tree. See the hand-over note, `docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md`, section « Décisions prises pendant l'exécution ».

**Goal:** the flash chip of the emulated device is written as on the device, and what is written can outlive a run. (1) ESP-IDF's unmodified `examples/storage/nvs/nvs_rw_value_cxx`, built from source, keeps a restart counter in NVS that increases across `esp_restart` inside one run: `The value is not initialized yet!`, then `Restart counter = 1`, then `= 2`. (2) A second `inkemu run` on the same `--flash-file` goes on counting where the first stopped. (3) The partition layout of the Xteink X4 (OTA slots, blank `otadata`) boots in `--strict-mmio`: the bootloader erases and programs `otadata` on the first boot only, with the measured 32 bytes, and both operations are counted. (4) In strict mode nothing unmodelled is touched and no CPU exception is taken over those lives. (5) Two runs from the same starting flash have the same trace hash and leave the same flash; the `hello_world` reference life keeps its trace hash `10174cc2726c3d77`. (6) The image given to `--flash` is never written; a run without `--flash-file` keeps nothing and says how many sectors were modified and not kept.

**Architecture:** The flash chip stays one component behind the SPI1 controller (`FlashDevice`, ADR 0002). The controller learns to send what its hard-wired commands carry (an address, a byte count, data) and runs exactly one command per write of `SPI1.CMD`; the chip model (`NorFlash`) learns to program (bitwise AND, wrap inside the 256-byte page), to erase a sector, a block or everything, and to take its status register, under the write-enable latch, which clears when an operation completes. Every operation completes at once. Whatever the model does not do is refused by name, never guessed. The chip counts a *generation* that moves only when a byte changes: it is the third term of `Bus::page_version`, so decoded code follows the content, and it tells the cache state of `EXTMEM` which 64 KiB flash pages a program or an erase changed. The cache windows stay a live view of the flash; two sets of 256 bits follow the one sequence in which a device would answer differently (a page read through a window, written, read again with no invalidation between) and the bus reports such a read. The machine performs no host I/O: it hands over the whole content as bytes and the number of 4 KiB sectors a run modified. The CLI turns that into a file: a raw image of exactly the flash size, read whole when a run starts, replaced through a temporary file and a rename when it ends, under a lock on a sibling file held for the whole run. The content a run starts from is an input of the run; its SHA-256 is printed. A second reference firmware, built by the same container run as the first, proves all of it on bytes Espressif's code writes.

**Tech Stack:** Rust 1.99.0 (edition 2024); no new dependency (`std::fs::File::try_lock` for the lock, `sha2` already in `emu-cli`); `clap`, `thiserror`, `assert_cmd`/`predicates`/`tempfile` (tests); the reference firmware tooling of plan SP1b-2 (`tools/reference-firmware`, Docker `espressif/idf:v5.5.2`), extended by Task 11.

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1, §4.1, §5.1, §5.2, §7, §10.2, §10.3). Hardware facts: `docs/research/esp-idf-storage-flash-traffic.md` (**SF**), `docs/research/flash-writes-and-crosspoint-boot.md` (**FW**), `docs/research/esp32c3-cache-mmu.md` (**CM**), and the reports of the earlier plans as their code cites them. ADR `docs/adr/0002-flash-model-and-cache-windows.md` (revised by Task 6), ADR `docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md` (written by Task 10). Carried-over points: `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` (**N3a**) and `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md` (**N2**). A value in this plan is followed by the report tag and section it comes from; `ROM3:name@addr` is a symbol of the rev3 mask ROM, `IDF:` a file of ESP-IDF v5.5.2.

Every code block of this plan was compiled and its tests run in a scratch clone of the repository, one commit per task, in the order of the tasks: after each task the workspace built, `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` were clean and `cargo test --workspace` passed. The ignored tests were run against the real ROM and the reference images after the tasks that say so. The numbers the steps state (test counts, hashes, console lines) are the ones that clone produced. Each behaviour a task claims names the mutation that must make its test fail; each mutation in the tables was applied to the clone and the test named beside it failed. A mutation that survived is listed as such, with the test that kills it elsewhere. Applied in order to the commit before, the blocks of each task rebuild the commit of that task exactly: this was checked mechanically for the 14 tasks.

**How to read the code blocks.** A block headed `--- a/… +++ b/…` is a unified diff against the file as the previous task left it: apply it with the editor, hunk by hunk (the `@@` line names the enclosing item; the unmarked lines are context to find the place). A block without that header is the whole content of a new file, or of the part of it the step names. A step that says "tests" gives the hunks inside the file's `#[cfg(test)] mod tests`; a step that says "code" gives the others.

## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. Workspace lints unchanged: `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests (an invariant is documented with `// INVARIANT:`).
- No `TODO`, no placeholder, no `#[allow]` added to hide a lint.
- Coverage ≥ 80 % for the workspace (`cargo llvm-cov --workspace --fail-under-lines 80`).
- Licence Apache-2.0; `cargo deny check` passes; no dependency is added.
- Register addresses and names come from `svd-regs` (`svd_regs::esp32c3::addr("PERIPHERAL", "REGISTER")`), never from a literal.
- Every user-facing message states **the cause, then the remedy**, separated by a semicolon.
- Untrusted input never panics and is bounded. The flash file is untrusted: its size is judged before anything is read, and no more than one byte past the flash size is ever read. What the firmware writes is untrusted: **no loop and no allocation is sized by a value the firmware wrote without a constant bound**; each task that takes such a value (a length, an address, a range, a count) says where its bound is.
- Determinism: same inputs → same trace hash and same final flash. No wall clock, no host randomness, no `HashMap` iteration on the emulated path. Saving the flash file happens after the run and enters no trace.
- `soc-esp32c3` performs **no host I/O** and builds for `wasm32-unknown-unknown` (`cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3`, a step of the CI job `check`). `emu-core` depends on no workspace crate.
- Third-party material (ROM, firmware, bootloader) stays out of the repository and of the binary, and is **never executed on the host**: inkemu is the only thing that executes a reference image.
- Each modelled hardware behaviour cites its source in a comment (report tag + section, ROM symbol, ESP-IDF file and line). For every hardware bit a model gives a meaning to, the comment names one call site in the ROM or in ESP-IDF that sets it. An assumed behaviour is marked `ASSUMPTION:` with what would settle it, and is listed in the hand-over note (Task 14).
- **A test proves only what a mutation makes fail.** Each task ends with a table of mutations: apply one, run the command, see the named test fail, undo it. Do not commit a task whose mutation survives.
- Code, tests, commit messages, the README, the ADRs and the CLI never cite the decisions table of this plan or anything this plan was written from: they cite reports, ADRs, the spec and the code. Only the hand-over note refers to the table (by its `G` numbers), as the note of SP1b-3a does.
- Source files are changed with the editor (read, then edit the named lines), never by a script that rewrites them.
- Checking a command: redirect its output to a file, then read `$?`; never read an exit code through a pipe. Use `/usr/bin/git`. Run Python with `-I`.
- Commits: `type(scope): description`, only the paths of the task (`/usr/bin/git commit -- <paths>`), message ending with the line `Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ`.
- Formatting and lints before **every** commit: `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`, all three `exit=0`, whether or not the task lists them. The code blocks of this plan are formatted as `rustfmt` leaves them.
- A task that changes a list of registers accepted without effect regenerates the table with `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect`, reads the diff of `docs/registers-accepted-without-effect.md`, and commits that file with the task.
- The ignored tests need `INKEMU_ROM_ELF_DIR` (the pinned esp-rom-elfs archive, `inkemu rom fetch --all --rom-dir <dir>`) and, for the reference images, `INKEMU_REF_FLASH`, `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH` (`flash_hello.bin`, `flash_nvs.bin` and `flash_nvs_ota.bin` of `tools/reference-firmware/build.sh`; the last two exist from Task 11 on). A task that says "ignored tests" runs `tools/ci/run-ignored-tests.sh` with the counts `.github/workflows/ci.yml` has at that point; a task that adds an ignored test updates the count in the same commit.
- **Docker** is needed by Task 11 only, once: `tools/reference-firmware/build.sh`. Never start two `docker run` at the same time.
- Exit codes of `inkemu run` stay 0, 2, 3, 4 and 130. No task adds one.
- Documents for people (the ADRs, the hand-over note) are in French, with all accents. README (English, then French), code, CLI output and this plan are in English.

## Review Focus

The input classes most likely to bite, each with the test that holds it and the mutation a final reviewer should try (apply, run the command of the task, see the test fail, undo):

- **Hostile lengths and addresses from the firmware.** A program of 0 or of more than 64 bytes; an address at or past the end of the chip; an erase whose unit crosses the end of a smaller chip; a read that starts inside and ends outside; a transfer whose address is not 24 bits wide; a cache invalidation of `0x7FFFFF` lines from the last MMU entry; a flash page number no MMU entry can name → `a_hard_wired_program_of_no_byte_or_of_more_than_64_is_refused_and_sends_nothing` (Task 2), `a_write_past_the_end_of_a_smaller_chip_is_refused_by_name` (Task 3), `a_read_that_reaches_past_the_end_of_the_chip_is_refused_and_reads_nothing` (Task 4), `a_request_that_names_no_line_of_a_window_drops_nothing`, `page_numbers_no_entry_can_name_are_in_no_set` (Task 6). Mutations: `address >= self.config.size` → `>`; drop `len > self.config.size - address`; in `Extmem::invalidate`, clear the two sets whenever the size is 0. (`.take(MMU_ENTRIES)` in that function is a bound, not a behaviour: without it the loop is still bounded by the 23 bits of the size field, at about 4,200 rounds, and no test can tell. Read it, do not mutate it.)
- **A program without the write-enable latch, and the latch after an operation.** The chip ignores the first and clears the second; ESP-IDF sends a write disable to a chip that leaves it set, which is the artefact of QEMU's chip → `a_program_needs_the_write_enable_latch_and_clears_it_when_it_completes`, `an_erase_without_the_latch_changes_nothing` (Task 3), `a_first_boot_sends_one_write_enable_per_write_and_the_application_no_write_disable` (Task 12). Mutation: remove `self.status &= !STATUS_WEL;` after the program.
- **Several command bits in one write of `SPI1.CMD`, and the `FLASH_PE` flag.** One command per write; the ROM's `SE | PE` is one erase, not two → `one_write_of_cmd_with_several_command_bits_runs_none_and_says_so`, `the_rom_sector_erase_sets_the_program_erase_flag_and_is_one_command` (Task 2). Mutation: run every set bit in a loop.
- **A write that changes nothing.** `0x00` over `0x00`, an erase of an erased sector: performed, in the journal, and it modifies nothing, invalidates nothing, makes nothing stale and makes no file be written → `an_operation_that_changes_no_byte_is_performed_and_modifies_nothing` (Task 3), `flash_written_before_a_window_read_it_or_left_as_it_was_is_not_reported` (Task 6), `a_first_run_creates_the_flash_file_and_the_next_one_goes_on_from_it` (Task 9). Mutation: `mark_modified` unconditionally.
- **A window read after a write.** The read returns the flash, and is reported once per page until the cache is invalidated for it; switching the cache off and on is no invalidation; a partial invalidation drops nothing → Task 6, all tests of `periph::extmem::tests` and `bus::tests` it adds; on real firmware `the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only`. Mutation: deliver `Event::CacheInvalidated` to nothing (the OTA boot then reports a stale read at the second boot).
- **A flash file of the wrong size, or that is not a flash file.** Shorter, longer, empty, sparse and huge, a directory, a symbolic link, a firmware image given to the wrong option, with and without `--reset-flash` → `a_file_of_another_size_is_refused_with_both_sizes_and_left_as_it_is`, `a_huge_file_is_refused_on_its_size_without_being_read`, `a_link_is_not_followed_and_what_it_points_to_is_never_written` (Task 8), `a_reset_does_not_replace_a_file_that_is_not_a_flash_of_the_device` (Task 9), `a_flash_file_of_another_size_is_refused_before_boot_with_both_sizes_and_left_as_it_is` (Task 10). Mutations: `metadata.len() != size` → `>`; `symlink_metadata` → `metadata`; skip `exists()` under a reset.
- **A directory that is read-only or gone when the run ends.** The run says what could not be done and what is lost, exits 2 (3 after a strict stop), and leaves no temporary file → `a_directory_that_vanished_fails_the_save_by_name_and_says_what_is_lost`, `a_save_that_cannot_take_the_name_leaves_the_old_content_and_no_temporary_file` (Task 8), `a_flash_file_that_cannot_be_written_is_said_in_the_log_and_answered_by_no` (Task 9), `a_flash_file_whose_directory_vanished_during_the_run_exits_2_and_says_what_is_lost`, `a_flash_file_whose_directory_became_read_only_exits_2_and_leaves_no_file` (Task 10). Mutation: ignore the answer of `RunFlash::finish` in `main.rs`.
- **Two processes on one file.** The second is refused before it loads a ROM, while the first runs; the first Ctrl-C saves → `a_second_run_on_the_same_file_is_refused_until_the_first_ends` (Task 8), `a_flash_file_that_another_run_holds_is_refused_before_boot`, `a_second_run_on_a_flash_file_is_refused_while_the_first_goes_on_and_ctrl_c_saves_it` (Task 10). Mutation: replace `lock.try_lock()` by `Ok(())`.
- **A seed next to a file that exists.** Refused before the run; `--reset-flash` starts again and replaces the file even when the run modified nothing; `--flash` is byte-identical after a run that wrote the chip → `a_seed_and_an_existing_flash_file_are_refused_unless_the_file_is_reset` (Task 9), `the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next`, `a_run_without_a_flash_file_says_what_it_does_not_keep_and_leaves_the_image_as_it_is` (Task 10). Mutation: `!reset && seed.is_some()` → `false && …`.
- **Determinism with a flash that is written.** Same starting content → same trace hash, same final content, twice; the trace hash alone does not tell two starting contents apart → `the_same_starting_flash_file_gives_the_same_trace_and_the_same_final_file` (Task 10), `three_lives_of_the_nvs_example_are_the_same_run_twice_and_leave_the_same_flash` (Task 12).
- **A proof that holds only inside the workspace.** `RomImage::from_flat` exists under the `test-support` feature, which another crate switches on in `cargo test --workspace` and nothing does in `cargo test -p soc-esp32c3`. A doc example that calls it passes in one and fails in the other; a `compile_fail` example that calls it passes in the second for the wrong reason, whatever it was written to refuse. The two doc examples of Task 7 take their `RomImage` as a parameter → run `cargo test -p soc-esp32c3 --doc` (2 pass). Mutation: `pub(crate) fn new` → `pub fn new` in `bus.rs`; the example of `SocBus::new` must then fail with "Test compiled successfully, but it's marked `compile_fail`".
- **What the reference firmware cannot tell.** Four mutations survive every test that runs Espressif's code, and are killed by unit tests only: a program that overwrites instead of AND-ing (NVS only clears bits), an erase that erases nothing in the OTA layout (the `otadata` sector is already erased), the third term of `page_version`, and an invalidation of a range. A reviewer who weakens `a_program_only_clears_bits`, `a_sector_erase_and_a_block_erase_set_their_whole_unit_and_nothing_else`, the two `code_decoded_from_the_flash_…` tests or `an_invalidation_drops_the_pages_of_the_whole_entries_it_covers_in_either_window` removes the only proof.

---

## Design decisions taken for this plan

G1–G18 were given to this plan and bind it. G19 onward were taken while writing it.

| # | Decision | Reason | Cost if wrong |
|---|---|---|---|
| G1 | **The chip accepts the write commands**: page program `02` (AND with the content, wrap inside the 256-byte page), sector erase `20`, block erase `D8`, chip erase `60`/`C7`, status write `01`, `WREN`/`WRDI`. A program or an erase without the write-enable latch is ignored, as the datasheets say. **The latch clears when an operation completes.** The 32 KiB erase `52` is not modelled: no source shows a caller. The artefact of QEMU's chip (a `WRDI` after each write) must not appear: Task 12 pins the commands of a first boot without it. | FW §C, §G, key result 5; SF §B.1. | Medium: the reference firmware cannot tell AND from overwrite; unit tests hold it. |
| G2 | **Operations complete at once**: the busy bit is never seen set. Real durations go to SP8. | FW §A.5: a model that completes every command at once satisfies ROM and ESP-IDF. | Low; a first OTA boot is about 100 ms shorter than on a device. |
| G3 | **The status register**: its writable bits are stored and read back; it is not part of the content and returns to its factory value with every new machine (a stated limit). A program or an erase while a block-protect bit is set is a named refusal: no protection map is modelled. | FW §C, §O.2: the part of a retail device is not established. | Low. |
| G4 | **Data reads through SPI1**: `03`, `0B`, `3B`, `BB`, `6B`, `EB`, each with a 24-bit address and its own dummy clocks. A read past the end, another address width or another dummy count is a named refusal. | FW §A.1, §P; SF §C: `BB`, 24-bit address, 4 dummy clocks, at most 64 bytes, is the only read measured. | Low. |
| G5 | **Hard-wired commands carry their address and data**: `FLASH_PP` (`ADDR = len << 24 \| addr24`, data in `W0..`), `SE`, `BE`, `CE`, `WRSR`, `READ`. `FLASH_PE` is a flag, not a command. The ROM's encoding (`SE \| PE`, 32-byte programs) and ESP-IDF's (`SE`, 64-byte programs) both write. **Exactly one chip operation per write of `CMD`.** | FW §A.1 to §A.3; SF §B.1, §D. | Medium. |
| G6 | **Auto-suspend is not modelled**: `FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL`, `SUS_STATUS` stay stored; a suspend or resume request is a named refusal; the three table rows go to SP8. | Suspending has a meaning only when an operation lasts (G2). | Low. |
| G7 | **`Bus::page_version` gets a third term**, the generation of the chip's content: a program or an erase that changes a byte drops the code decoded from the flash. | N3a; ADR 0002 point 5. | Low; one re-decode per write. |
| G8 | **The window stays a live view, and the read that may differ from silicon is named**: a 64 KiB flash page read through a window, then programmed or erased, then read again with no cache invalidation between. Two fixed sets of 256 bits, constant memory, one bit set and one bit tested on the window path. The message says "may differ". ROM, bootloader and ESP-IDF never raise it. A line-accurate cache goes to SP1b-4. ADR 0002 is revised. | CM §3; ADR 0002 "Écart connu". | Medium: see G26 for the limits. |
| G9 | **The content leaves the machine without host I/O**: `Esp32C3::flash_contents` and `flash_modified_sectors`. Loading and saving are the CLI's. A chip is built from `SocConfig::flash` and nowhere else. | N3a; N2 "à défaire" point 5; the wasm check of the CI. | Low. |
| G10 | **`--flash-file <path>` is opt-in**: a raw image of exactly the flash size of the profile, no header. Without it a run keeps nothing. | Spec §5.1; readable by `esptool.py`, comparable with `sha256sum`. | Low. |
| G11 | **Read at start, written at the end**: the file is read whole if it exists (another size is refused with both sizes, never padded or cut) and written when the run ends (a limit, the first Ctrl-C, a strict stop, an idle CPU) if it did not exist or the content was modified; not on a second Ctrl-C. Temporary file in the same directory, then a rename. A failed write is an error, never swallowed. A killed run loses its writes. | The pattern of the ROM cache; a run without I/O. | Medium: see G30, G32. |
| G12 | **`--flash` with a flash file that exists is refused** before the run (remedy: drop `--flash`, or add `--reset-flash`). `--reset-flash` starts from the seed or from an erased flash and replaces the file at the end; without `--flash-file` it is a usage error. `--flash` is never written. | A run cannot start from two flashes. | Low: see G29. |
| G13 | **An exclusive lock for the whole run** (`std::fs::File::try_lock`, no new dependency); a second process is refused with cause and remedy. | Two runs replacing one file would each lose the other's writes. | Low: see G28. |
| G14 | **The starting flash content is an input of a run**: the summary prints its SHA-256, and the final one when a file is written. Saving is never part of a trace. **ADR 0004** says what an input of a run is, the life of the file, and what the recording of SP1b-4 must hold. The bootloader ADR of SP1b-3c takes the next number. | Spec §3.1. | Low. |
| G15 | **Reference firmware**: `examples/storage/nvs/nvs_rw_value_cxx`, unmodified, built next to `hello_world` in the same container run, pinned: `nvs_rw_value_cxx.bin` `44523a2f…6516ff2`, `flash_nvs.bin` `c3f0eed1…8ec757f`, `flash_nvs_ota.bin` (behind `tests/fixtures/partitions/x4-stock.bin`) `87832c6d…4f28fec`, and its symbol table. The tests assert what SF §G lists. **No test asserts a count of `05`, nor an absolute count of `04`.** Covered by unit tests only, and said in the hand-over: an erase from an application, `D8`, 64-byte programs and the page-boundary split, `otadata` written by an application, the first NVS page change. | SF §A, §B, §G, §O.1. | Low. |
| G16 | **Persistence tests are hermetic**: temporary directories, the CLI through the binary as `run_cli.rs` does. One test "same starting file → same trace hash and same final file"; one that `--flash` is byte-identical after a run that wrote the chip. | Spec §8. | Low. |
| G17 | **Not in this plan**, and re-assigned by name in Task 1: `--firmware`, the bootloader ADR, partition tables in profiles, the form fixes of `boards` → SP1b-3c; UART reception, interrupts and frame format (8 table rows), USB-Serial-JTAG host side, the shared console test driver → SP1b-3d; CrossPoint → SP1b-3e; deep sleep, 10 power rows, `TIME_UPDATE` bits 29:27, the split of `timg.rs` and `rtc_cntl.rs`, light sleep refused by name → SP1b-3f; eFuse programming registers → no heir; the id type of `DeviceProfile` → SP2; real flash durations and suspend → SP8; a line-accurate cache → SP1b-4. Task 14 proves that nothing this plan did not do still names "SP1b-3b" as its heir. | One plan, one milestone. | Low. |
| G18 | **The spec is not edited.** Departures are in the ADRs and in the hand-over note, section « À reporter dans la spec ». | The spec is the user's document. | Low. |
| G19 | **Several command bits in one write of `SPI1.CMD` are refused by name** (`Unsupported::FlashCommandBits`), and none runs. Bits 17:0 are not commands: `FLASH_PE` is a flag, the rest are read-only state the SVD describes. | The order in which a real controller would run two commands is not documented; G5's "exactly one" is then true by construction. | Low: no ROM or ESP-IDF code sets two. |
| G20 | **SPI1 keeps a journal of what the chip wrote** (programs, erases, status writes; 1,024 entries, the overflow counted) and **counts the commands it sends by opcode** (256 counters). Both belong to the run: a reset of the controller keeps them. | The tests compare (address, bytes) lists and command lists with measurements; a front end can show them. Constant memory. | Low. |
| G21 | **"Modified" means a byte changed.** The generation and the modified-sector count move only then; a program of `0x00` over `0x00` is performed and journaled, and modifies nothing. | A file must not be rewritten, nor code re-decoded, nor a page made stale, by a write that changed nothing. | Low. |
| G22 | **Four named refusals of the chip**: an opcode it does not have, a frame it does not know (address width, dummy clocks, data), an address past its end, a block-protect bit set. A refusal reads nothing and changes nothing, the latch included. A write without the latch is ignored silently, even out of range: that is what the part does. | "Never guessed": a real part wraps or ignores the address bits it does not have (FW §C). | Low. |
| G23 | **The status write stores `value & 0xFC`**; `SRWD` locks nothing (`ASSUMPTION:` the level of `WP#` is not established). With `CTRL.WRSR_2B` the controller sends two bytes, as `ROM3:Enable_QMode` asks; the chip refuses the two-byte frame by name. | FW §C; `ROM3:Enable_QMode@0x4004d792`. | Low. |
| G24 | **The hard-wired `FLASH_READ` is modelled on an assumption** (address `ADDR[23:0]`, `MISO_DLEN` bytes into `W0..`): no ROM or ESP-IDF code writes that bit. | FW §A.1: "nobody". | Low: unreached by measured firmware. |
| G25 | **The six reads share one table of (opcode, dummy clocks)**; the quad reads answer whatever the quad-enable bit holds (`ASSUMPTION:`). | `IDF:spi_flash/include/spi_flash/spi_flash_defs.h:29-39, 60-71`. | Low. |
| G26 | **The stale state is kept per 64 KiB flash page, not per cache line.** A page leaves it when an invalidation covers a whole MMU entry that maps it, or covers everything (`Cache_Invalidate_ICache_Items(0, 0)`). A partial invalidation drops nothing; a size of 0 at another address, or an address outside the two windows, drops nothing (`ASSUMPTION:`). Switching the cache off and on drops nothing (`Cache_Suspend_ICache`). Limits, written in ADR 0002: silent for a page mapped at two addresses and invalidated at one; it speaks for a partial invalidation that covered the changed lines, for an evicted line, and for an entry remapped before it was invalidated. | G8's "constant memory, one bit test"; CM §4.1; `IDF:components/spi_flash/flash_mmap.c:299-326`. | Medium: SP1b-4 replaces it with real lines. |
| G27 | **`SocBus::new` and `Peripherals::new` become `pub(crate)`**, held by a `compile_fail` doctest: only `Esp32C3::new` and `with_flash` build a chip, from `SocConfig::flash`. | G9: by construction rather than by a check. | Low: nothing outside the crate called them. |
| G28 | **The lock is on a sibling file, `<path>.lock`, never deleted.** The flash file may not exist yet, and it is replaced by a rename: a lock on it would not survive. A symbolic link at the path is refused, not followed. | G13: "whichever survives the rename". | Low. |
| G29 | **A flash file of another size is refused even under `--reset-flash`**, which only replaces a file that is a flash of the device. Stricter than G12's wording. | A firmware image given to `--flash-file` by mistake must survive the remedy the user reaches for. | Low. |
| G30 | **A flash file that cannot be written at the end exits 2**, whatever ended the run, an idle CPU (4) included; a `--strict-mmio` stop keeps 3. The message is printed in every case. No new exit code. | The precedent of the console: `--strict-mmio` exists for CI and its code must not depend on a directory. | Low. |
| G31 | **The SHA-256 of the starting content is printed by every run**, with or without a flash file, after the summary of the run and after the trace hash. | G14. Measured: 12 ms for 16 MiB in a release build, 270 ms in a debug build. | Low. |
| G32 | **The file is saved for every end of a run**, the reader of standard output gone and a console error included. | What the firmware wrote is real whatever ended the run. | Low. |
| G33 | **ADR 0002 is revised as a whole by Task 6**, though its points 4 and 5 are true from Tasks 3 and 5. | One coherent revision instead of three partial ones; nothing reads the ADR between those tasks. | None. |
| G34 | **The flash file is three tasks**: the mechanism (`FlashState`, Task 8), the policy (`RunFlash`, Task 9), the options (Task 10). Until Task 10 nothing uses the first two. | Each has its own test cycle; no task leaves an option that lies. | None. |
| G35 | **The CI selects the tests of the reference firmware in `run_cli` by the name fragment `reference_firmware`** (Task 13). | A second firmware; the old filter named the first. | Low. |
| G36 | **The tests of the NVS lives do not assert the number of resets after the last life**: a run stops at the first slice boundary after `Restarting now.`, a few milliseconds before or after the restart. | Slice-dependent, not a device fact. | None. |
| G37 | **The trace hash is the path of the CPU, not its data**: two runs from two flash contents have the same trace hash when the firmware takes the same path. ADR 0004 says so and a test shows it. | Found while writing Task 10. | None. |

## Carried-over points: where they are handled

| Point (source) | Task |
|---|---|
| Flash writes: program, erases, status register; the latch (N3a « Reporté au plan SP1b-3b »; FW §G) | 3 |
| Address and data of the hard-wired commands; one command per write of `CMD` (N3a; FW §A) | 2 |
| The comment of `Spi1::run` and the test that named `USR \| FLASH_PE` "an ESP-IDF program" (N3a) | 2 |
| Data reads through SPI1 (`BB` first) (N3a; SF §C) | 4 |
| N2 hypothesis 13, the address of a user transfer taken unshifted: sourced for 24 bits, no longer an `ASSUMPTION:` | 4 |
| The two ignored tests of the OTA layout, whose premise (a refused `otadata` update) goes away | 3 |
| Third term of `Bus::page_version` (N3a; ADR 0002 point 5) | 5 |
| Live view or stale lines (ADR 0002 « Écart connu ») | 6 |
| The content given back by the machine, the "modified" state (N3a) | 7 |
| The size rule of an external `FlashDevice` (N2 « à défaire » point 5) | 7 |
| The device's flash file (spec §5.1; N3a) | 8, 9, 10 |
| A from-source firmware that writes (SF §G) | 11, 12, 13 |
| `SPI1.FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL`, `SUS_STATUS` (table rows) | 2 (to SP8, with the reason) |
| Everything else N3a, the table, the comments and the README hand to "SP1b-3b" | 1 (re-assigned by name), 14 (proved) |

## File Structure

```
.github/workflows/ci.yml                     counts and filters of the ignored tests; the NVS images (Tasks 10 to 13)
README.md                                    heirs (Task 1); the flash is written, the flash file (Task 14)
docs/adr/0002-flash-model-and-cache-windows.md  revised (Task 6)
docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md  new, in French (Task 10)
docs/registers-accepted-without-effect.md    regenerated (Tasks 1, 2, 6)
docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md  the re-assignment table (Task 1)
docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md  the hand-over note, in French (Task 14)
crates/soc-esp32c3/src/
  flash.rs            FlashAddress (Task 2); FlashRefusal, FlashWrite, FlashJournal, the write commands,
                      generation, modified sectors (Task 3); the data reads (Task 4); read by slice (Task 7)
  periph/spi_mem.rs   hard-wired commands, one per write, suspend refused (Task 2); the journal (Task 3);
                      Event::FlashWritten (Task 6); command counters (Task 12)
  diag.rs             Unsupported::{FlashProgramLength, FlashCommandBits, FlashSuspend} (Task 2),
                      {FlashCommandForm, FlashRange, FlashProtected} (Task 3); Diagnostic::StaleFlashWindow (Task 6)
  bus.rs              page_version third term (Task 5); the stale read reported (Task 6); new is pub(crate) (Task 7)
  ctx.rs              Event::{FlashWritten, CacheInvalidated} (Task 6)
  periph/extmem.rs    the cache state per flash page: window_read, flash_written, invalidate (Task 6)
  periph/mmu.rs       MmuTable::flash_page (Task 6)
  periph/mod.rs       delivery of the two events (Task 6); new is pub(crate) (Task 7)
  machine.rs          take_flash_writes, flash_writes_dropped (Task 3); flash_contents,
                      flash_modified_sectors (Task 7); flash_commands_sent (Task 12)
  test_support.rs     flash_program, flash_erase_sector (Task 3)
  lib.rs              re-export of FlashWrite (Task 3)
  periph/{uart,rtc_cntl,system,efuse,usb_serial_jtag}.rs   heirs renamed (Task 1)
crates/soc-esp32c3/tests/
  reference_boot.rs   the OTA tests rewritten (Task 3); the NVS lives (Task 12)
  accepted_without_effect.rs   no register is handed to a plan that is done (Task 14)
crates/emu-cli/src/
  flash_state.rs      new: FlashState (Task 8), RunFlash (Task 9)
  lib.rs              the module (Task 8)
  run.rs              journal prefix of the new diagnostic (Task 6); RunEnd::exit_code_without_its_flash (Task 9)
  exit.rs             the help of exit code 3 (Task 6); the test of the new exit rule (Task 9); the help of exit code 2 (Task 10)
  main.rs             the help of --strict-mmio (Task 6); --flash-file, --reset-flash, the summary lines, the exit code (Task 10)
crates/emu-cli/tests/
  run_cli.rs          the help of --strict-mmio (Task 6); the options through the binary (Task 10);
                      the NVS firmware through the binary (Task 13)
tools/reference-firmware/
  build.sh, in-container.sh, README.md, SHA256SUMS.output, SHA256SUMS.inputs-nvs   the second example (Task 11)
```

---
## Task 1: What was handed to "SP1b-3b" is shared between five plans, each named

The hand-over note of SP1b-3a, the table of registers accepted without effect, several comments and the README hand to one plan, "SP1b-3b", what five plans now share. This task re-assigns by name everything this plan does not do, before anything else changes, so that no later task works under a wrong label. It changes no behaviour.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/uart.rs`, `usb_serial_jtag.rs`, `rtc_cntl.rs`, `system.rs`, `efuse.rs` (heirs and comments)
- Modify: `docs/registers-accepted-without-effect.md` (its prose, then its table regenerated)
- Modify: `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` (one section added at the top; the rest is a record and is not rewritten)
- Modify: `README.md` (English, then French)

**Interfaces:**
- Consumes: `Accepted::svd(peripheral, register, not_modeled, heir)` and the table test `crates/soc-esp32c3/tests/accepted_without_effect.rs`.
- Produces: no code interface. The heirs `"SP1b-3d"` (8 UART rows) and `"SP1b-3f"` (10 power rows); the three `SPI1` rows keep `"SP1b-3b"` until Task 2.

- [ ] **Step 1: Count what names "SP1b-3b" today**

```bash
/usr/bin/git grep -n "SP1b-3b" -- crates > /tmp/t1-before.txt; wc -l < /tmp/t1-before.txt
grep -c "| SP1b-3b |" docs/registers-accepted-without-effect.md
```

Expected: `21` and `21`. The 21 lines of `crates` are: the eFuse module comment, 9 rows of `rtc_cntl.rs`, 3 rows of `spi_mem.rs`, 1 row of `system.rs`, 4 lines of `uart.rs`, 2 of `usb_serial_jtag.rs`, 1 comment of `tests/reference_boot.rs`. This plan keeps four of them for now: the three `SPI1` rows (Task 2 hands them to SP8) and the comment of `reference_boot.rs` (Task 3 rewrites the test it describes).

- [ ] **Step 2: Rename the heirs in the code**

The UART rows and comments go to SP1b-3d (the serial consoles in both directions), the power rows to SP1b-3f (deep sleep), and the eFuse programming registers to no plan.

````diff
--- a/crates/soc-esp32c3/src/periph/uart.rs
+++ b/crates/soc-esp32c3/src/periph/uart.rs
@@ -3,7 +3,7 @@
 //! A byte written to the FIFO leaves at the baud rate: the transmit FIFO fills, `STATUS` and
 //! `FSM_STATUS` say so, and the byte reaches the console when its last bit has gone. Software waits
 //! on both: the ROM before each character, ESP-IDF before it changes a clock or restarts (report IR
-//! §4.1). Reception and the UART interrupts arrive in SP1b-3b.
+//! §4.1). Reception and the UART interrupts arrive in SP1b-3d.
 use crate::clock::XTAL_PERIOD;
 use crate::ctx::{Ctx, Deadline, Event};
 use crate::diag::Unsupported;
@@ -24,7 +24,7 @@ pub(crate) fn accepted() -> Vec<Accepted> {
             ),
             ("CONF1", "FIFO thresholds"),
         ] {
-            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3b"));
+            rows.push(Accepted::svd(peripheral, register, not_modeled, "SP1b-3d"));
         }
     }
     rows
@@ -79,7 +79,7 @@ const SCLK_SEL_XTAL: u32 = 3;
 const TXFIFO_RST: u32 = 1 << 18;
 /// ASSUMPTION: a byte is 10 bit times on the wire (a start bit, 8 data bits, a stop bit), whatever
 /// `CONF0` says. The ROM and ESP-IDF consoles use this format; another one would only change how
-/// long a byte takes. SP1b-3b gives the frame format its effect, with the receive side; a byte
+/// long a byte takes. SP1b-3d gives the frame format its effect, with the receive side; a byte
 /// timed on a chip configured for another format (two stop bits, a parity bit) settles what each
 /// field adds.
 const BITS_PER_BYTE: u64 = 10;
@@ -482,7 +482,7 @@ mod tests {
         assert_eq!(
             uart.read(ctx, reg("INT_RAW")),
             None,
-            "interrupts are SP1b-3b"
+            "interrupts are SP1b-3d"
         );
         assert_eq!(uart.read(ctx, reg("INT_ST")), None);
         assert_eq!(
````

````diff
--- a/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
+++ b/crates/soc-esp32c3/src/periph/usb_serial_jtag.rs
@@ -1,7 +1,7 @@
 //! USB-Serial-JTAG, device-to-host serial path. Register layout: `usb_serial_jtag_reg.h` (report §G.3).
 //!
 //! The model behaves like a board with a host attached and reading: the IN endpoint always has
-//! room. Reception, interrupts and host attach/detach arrive in SP1b-3b.
+//! room. Reception, interrupts and host attach/detach arrive in SP1b-3d.
 use crate::ctx::Ctx;
 use emu_core::Peripheral;
 
@@ -131,7 +131,7 @@ mod tests {
         assert_eq!(
             usb.read(ctx, 0x08),
             None,
-            "INT_RAW arrives with interrupts (SP1b-3b)"
+            "INT_RAW arrives with interrupts (SP1b-3d)"
         );
         assert!(!usb.write(ctx, 0x18, 0), "CONF0");
         assert_eq!(usb.read(ctx, 0x00), None, "no host-to-device data yet");
````

````diff
--- a/crates/soc-esp32c3/src/periph/rtc_cntl.rs
+++ b/crates/soc-esp32c3/src/periph/rtc_cntl.rs
@@ -158,22 +158,22 @@ const ACCEPTED: [Accepted; 18] = [
         "RTC_CNTL",
         "OPTIONS0",
         "BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits act)",
-        "SP1b-3b",
+        "SP1b-3f",
     ),
     Accepted::svd(
         "RTC_CNTL",
         "TIMER1",
         "clock and PLL wake-up delays",
-        "SP1b-3b",
+        "SP1b-3f",
     ),
-    Accepted::svd("RTC_CNTL", "TIMER3", "Wi-Fi/BT power-up timing", "SP1b-3b"),
-    Accepted::svd("RTC_CNTL", "TIMER4", "digital power-up timing", "SP1b-3b"),
-    Accepted::svd("RTC_CNTL", "TIMER5", "minimum sleep time", "SP1b-3b"),
+    Accepted::svd("RTC_CNTL", "TIMER3", "Wi-Fi/BT power-up timing", "SP1b-3f"),
+    Accepted::svd("RTC_CNTL", "TIMER4", "digital power-up timing", "SP1b-3f"),
+    Accepted::svd("RTC_CNTL", "TIMER5", "minimum sleep time", "SP1b-3f"),
     Accepted::svd(
         "RTC_CNTL",
         "TIMER6",
         "peripheral power-up timing",
-        "SP1b-3b",
+        "SP1b-3f",
     ),
     Accepted::svd(
         "RTC_CNTL",
@@ -198,7 +198,7 @@ const ACCEPTED: [Accepted; 18] = [
         "RTC_CNTL",
         "DIG_PWC",
         "digital power-domain control",
-        "SP1b-3b",
+        "SP1b-3f",
     ),
     Accepted::svd(
         "RTC_CNTL",
@@ -213,8 +213,8 @@ const ACCEPTED: [Accepted; 18] = [
         "SP2",
     ),
     Accepted::svd("RTC_CNTL", "SENSOR_CTRL", "SAR ADC power forcing", "SP2"),
-    Accepted::svd("RTC_CNTL", "PWC", "RTC power-domain control", "SP1b-3b"),
-    Accepted::svd("RTC_CNTL", "DIG_ISO", "digital pad isolation", "SP1b-3b"),
+    Accepted::svd("RTC_CNTL", "PWC", "RTC power-domain control", "SP1b-3f"),
+    Accepted::svd("RTC_CNTL", "DIG_ISO", "digital pad isolation", "SP1b-3f"),
     Accepted::svd("RTC_CNTL", "PAD_HOLD", "pad hold", "SP2"),
     Accepted::svd("RTC_CNTL", "DIG_PAD_HOLD", "digital pad hold", "SP2"),
 ];
````

````diff
--- a/crates/soc-esp32c3/src/periph/system.rs
+++ b/crates/soc-esp32c3/src/periph/system.rs
@@ -133,7 +133,7 @@ const ACCEPTED: [Accepted; 9] = [
         "SYSTEM",
         "MEM_PD_MASK",
         "memory power-down mask in light sleep",
-        "SP1b-3b",
+        "SP1b-3f",
     ),
 ];
 
````

````diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -3,8 +3,9 @@
 //! (`svd_regs::esp32c3`, peripheral `EFUSE`) and the one hole the application reads (`0x18C`, see
 //! `RD_REPEAT_ERR_GAP`): the other reserved holes (such as 0x194..=0x1BC) are not registers, so they
 //! stay unmodelled and surface as diagnostics (spec §5.2).
-//! The content is that of a production chip (decision D9); programming registers stay unmodelled
-//! until a firmware needs them (SP1b-3b).
+//! The content is that of a production chip (decision D9). The programming registers (`CMD`,
+//! `PGM_*`) stay unmodelled and are reported by name; no plan is their heir, because no measured
+//! boot touches them (report FW §K.3 lists what the community firmware reaches before its hang).
 use crate::chip::ChipRevision;
 use crate::ctx::Ctx;
 use crate::periph::EFUSE_BASE;
````

- [ ] **Step 3: The table: its prose by hand, its rows regenerated**

Edit the prose above the `<!-- generated table -->` marker, then regenerate the rows:

```bash
INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t1-bless.log 2>&1; echo "exit=$?"
/usr/bin/git diff --stat -- docs/registers-accepted-without-effect.md
```

Expected: `exit=0`, and the file as this diff leaves it (18 rows change their last column):

````diff
--- a/docs/registers-accepted-without-effect.md
+++ b/docs/registers-accepted-without-effect.md
@@ -31,18 +31,19 @@ then rewrites the table below, and nothing else; review the diff before committi
 
 "Count" is 1 for a single register, or the number of consecutive registers the row stands for.
 "Made real by" names the plan that gives the register its effect. Each plan inherits the rows that
-name it. The table has six heirs: SP1b-3b, SP1b-4, SP2, SP5a, SP5b and SP8. The carry-over note
-`docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` names the rows of each heir one by one, as
-the table stood at the end of plan SP1b-3a, in its section "Lignes de la table des registres
-acceptés, par héritier".
+name it. The table has eight heirs: SP1b-3b (flash writes), SP1b-3d (the serial consoles in both
+directions), SP1b-3f (deep sleep), SP1b-4, SP2, SP5a, SP5b and SP8. The carry-over note
+`docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` names the rows one by one, as the table
+stood at the end of plan SP1b-3a, when one plan, "SP1b-3b", was to take what five plans now share;
+its first section says which plan takes what.
 
 <!-- generated table -->
 | Address | Register | Count | Hardware effect not modelled | Made real by |
 |---|---|---|---|---|
-| `0x6000000c` | `UART0.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3b |
-| `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3b |
-| `0x60000020` | `UART0.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3b |
-| `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3b |
+| `0x6000000c` | `UART0.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
+| `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
+| `0x60000020` | `UART0.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
+| `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3d |
 | `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level | SP8 |
 | `0x6000200c` | `SPI1.CTRL1` | 1 | CS hold delay after a release-from-power-down | SP8 |
 | `0x60002010` | `SPI1.CTRL2` | 1 | CS setup and hold times | SP8 |
@@ -73,19 +74,19 @@ acceptés, par héritier".
 | `0x60004168` | `GPIO.FUNC5_IN_SEL_CFG` | 1 | input signal routing (GPIO matrix) | SP2 |
 | `0x600050f0` | `FE2.FE2_TX_INTERP_CTRL` | 1 | radio memory power: there is no radio | SP5a |
 | `0x60006090` | `FE.FE_GEN_CTRL` | 1 | radio memory power: there is no radio | SP5a |
-| `0x60008000` | `RTC_CNTL.OPTIONS0` | 1 | BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits act) | SP1b-3b |
-| `0x6000801c` | `RTC_CNTL.TIMER1` | 1 | clock and PLL wake-up delays | SP1b-3b |
-| `0x60008024` | `RTC_CNTL.TIMER3` | 1 | Wi-Fi/BT power-up timing | SP1b-3b |
-| `0x60008028` | `RTC_CNTL.TIMER4` | 1 | digital power-up timing | SP1b-3b |
-| `0x6000802c` | `RTC_CNTL.TIMER5` | 1 | minimum sleep time | SP1b-3b |
-| `0x60008030` | `RTC_CNTL.TIMER6` | 1 | peripheral power-up timing | SP1b-3b |
+| `0x60008000` | `RTC_CNTL.OPTIONS0` | 1 | BBPLL, crystal and I2C power forcing, CPU stall (the two software reset bits act) | SP1b-3f |
+| `0x6000801c` | `RTC_CNTL.TIMER1` | 1 | clock and PLL wake-up delays | SP1b-3f |
+| `0x60008024` | `RTC_CNTL.TIMER3` | 1 | Wi-Fi/BT power-up timing | SP1b-3f |
+| `0x60008028` | `RTC_CNTL.TIMER4` | 1 | digital power-up timing | SP1b-3f |
+| `0x6000802c` | `RTC_CNTL.TIMER5` | 1 | minimum sleep time | SP1b-3f |
+| `0x60008030` | `RTC_CNTL.TIMER6` | 1 | peripheral power-up timing | SP1b-3f |
 | `0x60008034` | `RTC_CNTL.ANA_CONF` | 1 | analog configuration, glitch reset | SP8 |
 | `0x60008070` | `RTC_CNTL.CLK_CONF` | 1 | RTC fast clock source and dividers (a slow clock other than the RC oscillator is refused) | SP8 |
 | `0x60008074` | `RTC_CNTL.SLOW_CLK_CONF` | 1 | RTC slow clock divider | SP8 |
 | `0x60008080` | `RTC_CNTL.RTC_CNTL` | 1 | RTC regulator and digital bias | SP8 |
-| `0x60008084` | `RTC_CNTL.PWC` | 1 | RTC power-domain control | SP1b-3b |
-| `0x60008088` | `RTC_CNTL.DIG_PWC` | 1 | digital power-domain control | SP1b-3b |
-| `0x6000808c` | `RTC_CNTL.DIG_ISO` | 1 | digital pad isolation | SP1b-3b |
+| `0x60008084` | `RTC_CNTL.PWC` | 1 | RTC power-domain control | SP1b-3f |
+| `0x60008088` | `RTC_CNTL.DIG_PWC` | 1 | digital power-domain control | SP1b-3f |
+| `0x6000808c` | `RTC_CNTL.DIG_ISO` | 1 | digital pad isolation | SP1b-3f |
 | `0x600080d0` | `RTC_CNTL.PAD_HOLD` | 1 | pad hold | SP2 |
 | `0x600080d4` | `RTC_CNTL.DIG_PAD_HOLD` | 1 | digital pad hold | SP2 |
 | `0x600080d8` | `RTC_CNTL.BROWN_OUT` | 1 | brown-out detector: the supply is always good | SP2 |
@@ -98,10 +99,10 @@ acceptés, par héritier".
 | `0x6000e040` | `ANA_I2C.I2C_MST_ANA_CONF0` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
 | `0x6000e044` | `ANA_I2C.ANA_CONFIG` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
 | `0x6000e048` | `ANA_I2C.ANA_CONFIG2` | 1 | analog register bus: bytes are stored, the analog blocks (PLL, SAR ADC, bias) do nothing | SP2 |
-| `0x6001000c` | `UART1.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3b |
-| `0x60010010` | `UART1.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3b |
-| `0x60010020` | `UART1.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3b |
-| `0x60010024` | `UART1.CONF1` | 1 | FIFO thresholds | SP1b-3b |
+| `0x6001000c` | `UART1.INT_ENA` | 1 | interrupt enables: nothing is raised | SP1b-3d |
+| `0x60010010` | `UART1.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
+| `0x60010020` | `UART1.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
+| `0x60010024` | `UART1.CONF1` | 1 | FIFO thresholds | SP1b-3d |
 | `0x6001ccd4` | `NRX.NRXPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
 | `0x6001d054` | `BB.BBPD_CTRL` | 1 | radio memory power: there is no radio | SP5a |
 | `0x60026014` | `APB_CTRL.WIFI_CLK_EN` | 1 | radio clock gates: there is no radio | SP5a |
@@ -118,7 +119,7 @@ acceptés, par héritier".
 | `0x600c0000` | `SYSTEM.CPU_PERI_CLK_EN` | 1 | clock gates of the CPU peripherals: a gated module keeps answering | SP8 |
 | `0x600c0004` | `SYSTEM.CPU_PERI_RST_EN` | 1 | resets of the CPU peripherals (the debug assistant) | SP1b-4 |
 | `0x600c0008` | `SYSTEM.CPU_PER_CONF` | 1 | PLL_FREQ_SEL and the CPU wait-mode bits (CPUPERIOD_SEL is used by the clock model) | SP8 |
-| `0x600c000c` | `SYSTEM.MEM_PD_MASK` | 1 | memory power-down mask in light sleep | SP1b-3b |
+| `0x600c000c` | `SYSTEM.MEM_PD_MASK` | 1 | memory power-down mask in light sleep | SP1b-3f |
 | `0x600c0010` | `SYSTEM.PERIP_CLK_EN0` | 1 | peripheral clock gates: a gated module keeps answering | SP8 |
 | `0x600c0014` | `SYSTEM.PERIP_CLK_EN1` | 1 | peripheral clock gates: a gated module keeps answering | SP8 |
 | `0x600c0020` | `SYSTEM.BT_LPCK_DIV_INT` | 1 | Bluetooth low-power clock divider: there is no radio | SP5b |
````

- [ ] **Step 4: The note of SP1b-3a says who takes what**

One section is added under the title; nothing else of the note changes: it stays the record of what was decided at the end of SP1b-3a.

````diff
--- a/docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md
+++ b/docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md
@@ -2,6 +2,25 @@
 
 Note de passage du plan `docs/superpowers/plans/2026-10-11-sp1b-3a-interrupts-timers-reset.md` (base de temps, interruptions, timers, watchdogs, reset) aux plans suivants. Elle garde ce que l'historique git ne dit pas : les décisions prises en cours de route, et ce qui est volontairement reporté, avec le plan qui en hérite.
 
+## Réattribution du 12 octobre 2026 : « SP1b-3b » est devenu cinq plans
+
+Cette note, la table des registres acceptés et plusieurs commentaires du code attribuaient à un seul plan, « SP1b-3b », ce que cinq plans se partagent désormais. Le texte ci-dessous n'est pas réécrit : il reste le relevé de ce qui était décidé à la fin de SP1b-3a. Partout où il dit « SP1b-3b », lire le plan de ce tableau.
+
+| Ce que la note attribue à « SP1b-3b » | Plan qui le prend |
+|---|---|
+| Écritures en flash (programmation, effacements, registre d'état), lectures de données par SPI1, adresse et données des commandes câblées, mise à jour d'`otadata` et comptage des écritures dans le test OTA | **SP1b-3b** (écritures en flash et fichier de flash de l'appareil) |
+| Fichier de flash qui survit aux exécutions, contenu rendu par la machine, état « modifié », troisième terme de `Bus::page_version`, vue en direct ou lignes de cache périmées (ADR 0002), règle de taille d'une `FlashDevice` externe | **SP1b-3b** |
+| Lignes `SPI1.FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL` et `SUS_STATUS` de la table | **SP8**, par décision du plan SP1b-3b : l'attente automatique et la suspension n'ont de sens que lorsqu'une opération de flash dure |
+| `--firmware`, l'ADR sur le bootloader qui doit le précéder, les tables de partitions dans les profils, les retouches de forme de `boards` (`# Errors` de `from_named_toml`, `ProfileError::Parse`) | **SP1b-3c** |
+| Réception UART, interruptions UART, format de trame (les 8 lignes `INT_ENA`, `INT_CLR`, `CONF0`, `CONF1` des deux UART), côté hôte de l'USB-Serial-JTAG, pilote de console commun aux tests d'intégration et leur accès à `test_support` | **SP1b-3d** (les consoles série dans les deux sens) |
+| Le jalon « un firmware communautaire démarre » (CrossPoint) et son pilote de test | **SP1b-3e** |
+| Sommeil profond et réveil par le timer RTC, les 10 lignes d'alimentation (`RTC_CNTL.OPTIONS0`, `TIMER1`, `TIMER3` à `TIMER6`, `PWC`, `DIG_PWC`, `DIG_ISO`, `SYSTEM.MEM_PD_MASK`), les bits 29:27 de `RTC_CNTL.TIME_UPDATE`, le découpage de `periph/timg.rs` et de `periph/rtc_cntl.rs` | **SP1b-3f** |
+| Sommeil léger | **SP1b-3f**, qui le refuse en le nommant : la spec ne demande que le sommeil profond et le réveil par timer (§10.2, point 7) |
+| Registres de programmation des eFuse (`EFUSE.CMD`, `PGM_*`) | Aucun plan : ils restent des accès non modélisés, signalés par leur nom ; aucun démarrage mesuré ne les touche |
+| Identifiant de `DeviceProfile` comme type validé | **SP2** : aucun fichier ne porte plus le nom d'un profil, le fichier de flash est désigné par son chemin |
+| Durées réelles des opérations de flash, suspension d'un effacement ou d'une programmation | **SP8** |
+| Cache fidèle à la ligne (une ligne lue avant une écriture reste périmée jusqu'à son invalidation) | **SP1b-4**, avec le coût d'un défaut de cache (`Bus::fetch_stall`) |
+
 ## Résultat
 
 L'image de référence (bootloader ESP-IDF v5.5.2 et `hello_world`, mêmes empreintes que pour SP1b-2) vit toute sa vie sur la vraie ROM : `Hello world!`, onze lignes de compte à rebours espacées d'une seconde d'appareil, `Restarting now.`, puis un second démarrage que la ROM annonce par `rst:0xc (RTC_SW_CPU_RST)` et `Saved PC:0x40048b92`. Une vie, de la mise sous tension au reset du CPU par `esp_restart`, représente 28 347 166 cycles de CPU pour 11,246 s d'appareil (mesuré avec la CLI, non figé par un test : les tests figent le jalon, onze lignes de compte à rebours espacées d'une seconde d'appareil au tick près et un second démarrage derrière `rst:0xc`, et, par la commande réelle, la même console et la même empreinte de trace pour deux exécutions menées à travers le redémarrage) ; le reste est dormi. En `--strict-mmio`, aucun registre non modélisé et aucune exception sur tout le cycle. Sans image, la puce est réinitialisée par ses watchdogs à 1,3 s, 2,6 s (`rst:0x7`) puis 2,941 s (`rst:0x10`), et le cycle recommence. Un firmware qui se bloque, interruptions masquées, est réinitialisé par son propre watchdog, et le démarrage suivant donne l'adresse où il était bloqué.
````

- [ ] **Step 5: The README names the plans**

````diff
--- a/README.md
+++ b/README.md
@@ -2,7 +2,7 @@
 
 **A binary-level emulator for embedded e-ink devices.** inkemu runs the real, unmodified firmware image of a device on a virtual replica of its hardware. You can check how a firmware renders and behaves before you flash it.
 
-> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Flash writes and a firmware given on its own come next (sub-project 1b-3b), then the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
+> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Flash writes and the device's flash file come next (sub-project 1b-3b), then a firmware given on its own (1b-3c), the serial consoles in both directions (1b-3d), a community firmware (1b-3e), deep sleep (1b-3f) and the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
 
 ## Build & try
 
@@ -48,7 +48,7 @@ $ target/release/inkemu run --device x4 --flash app.bin
 error: app.bin: the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)
 ```
 
-Without `--flash` the flash is erased and the ROM prints `invalid header: 0xffffffff` in a loop, as a blank chip does. There is no option yet to boot an application `.bin` on its own: inkemu does not compose a flash from one (planned for sub-project 1b-3b).
+Without `--flash` the flash is erased and the ROM prints `invalid header: 0xffffffff` in a loop, as a blank chip does. There is no option yet to boot an application `.bin` on its own: inkemu does not compose a flash from one (planned for sub-project 1b-3c).
 
 The reference image (`hello_world` of ESP-IDF v5.5.2, saved as `flash_hello.bin`) runs its whole life: it prints `Hello world!`, counts down eleven seconds, calls `esp_restart()`, and the ROM boots again, announcing `rst:0xc (RTC_SW_CPU_RST)`. The seconds the application spends waiting are not executed: the CPU sleeps in `wfi` and device time jumps to the next timer event, so the eleven seconds take about one second of host time. With `--max-cycles 50000000`, standard error reads:
 
@@ -76,9 +76,9 @@ Things worth knowing:
 - **Two limits, two clocks.** `--max-cycles N` stops the run when the CPU has executed N cycles of its clock; `--max-time S` stops it when S seconds of device time have elapsed since power-on (a decimal number: `30`, `2.5`, `0.001`). `--max-cycles` counts the cycles the CPU executes, not device time: a sleeping CPU executes none while device time goes on, as it does across resets. A firmware that sleeps therefore reaches that limit late (the reference image waits on a timer for eleven seconds and still stops at `--max-cycles 50000000`), and one whose sleep nothing interrupts never does; `--max-time` bounds both. Both may be given; the first one reached ends the run, with exit code 0. The stop is exact and the same on every host: the first instruction boundary at or after the limit, or the limit itself when the CPU is asleep. A firmware that waits for an interrupt nothing can raise ends the run by itself, with exit code 4.
 - **Ctrl-C and `kill`.** Without a limit a run ends with Ctrl-C, which prints the same summary as a limit, with exit code 0. A second Ctrl-C before the run has stopped exits at once, with code 130 and no summary. A plain `kill` is not handled and ends the process without a summary either. With `--trace-hash`, the hash of an interrupted run covers what ran up to the cut, which depends on host timing: it is not reproducible; the hash of a run ended by a limit is.
 - **The watchdogs are real.** Without `--flash`, the ROM prints `invalid header: 0xffffffff` until the watchdog the chip arms at reset expires, 1.3 s later, and boots again with `rst:0x7 (TG0WDT_SYS_RST)`, as a blank chip does. A firmware that stops feeding its watchdogs is reset the same way, and the next boot names the watchdog and the address it was caught at (`Saved PC`). inkemu's own `reset by …` line gives the cause, the program counter and the stack pointer of every reset, also after a system reset, where the ROM prints no `Saved PC`.
-- **The console is as slow as the UART, and output only.** A byte leaves at the baud rate the firmware programmed; when a run stops, what was still in the UART is printed too. `--console` chooses which serial output of the device goes to standard output: `uart0` (the default), `uart1` or `usb-serial-jtag`; the ROM prints on the first and on the last. Nothing can be sent to the device yet: there is no UART reception and no host side of USB-Serial-JTAG (sub-project 1b-3b).
+- **The console is as slow as the UART, and output only.** A byte leaves at the baud rate the firmware programmed; when a run stops, what was still in the UART is printed too. `--console` chooses which serial output of the device goes to standard output: `uart0` (the default), `uart1` or `usb-serial-jtag`; the ROM prints on the first and on the last. Nothing can be sent to the device yet: there is no UART reception and no host side of USB-Serial-JTAG (sub-project 1b-3d).
 - **Pipes and redirections.** `inkemu run … | head -5` ends when `head` has its lines: a run whose reader went away says so and ends with exit code 0. On Unix this also holds when the firmware prints nothing more: every so often, after a slice that had nothing to print, inkemu asks whether its reader is still there, and a reader that left is noticed after 64 such slices at most (a few milliseconds to half a second on the machine it was measured on, depending on what the firmware does). A run that ends by itself in the meantime ends as what it is, with the exit code of that end. On other systems such a silent run goes on to its limit or to Ctrl-C. When standard output cannot be written for another reason (a full disk), the run ends with exit code 2; a `--strict-mmio` stop keeps its code whatever happened to the console. A closed standard output (`>&-`) cannot be told from `/dev/null`, because the Rust runtime reopens it on the null device before the program starts: it behaves like `/dev/null`, the console is dropped and the run ends as it would have, with exit code 0.
-- **What is not emulated yet.** Deep sleep and light sleep: a firmware that asks for either keeps running (sub-project 1b-3b). There is no radio, no display and no button yet. The first line of standard error says `timing model datasheet-uncalibrated (not calibrated)`: the cycle cost of each instruction comes from documents, not from a measurement on a device, so a device time is plausible, not exact (the calibration bench is sub-project 8).
+- **What is not emulated yet.** Deep sleep and light sleep: a firmware that asks for either keeps running. Sub-project 1b-3f models deep sleep and the wake-up by the RTC timer, and refuses light sleep by name. There is no radio, no display and no button yet. The first line of standard error says `timing model datasheet-uncalibrated (not calibrated)`: the cycle cost of each instruction comes from documents, not from a measurement on a device, so a device time is plausible, not exact (the calibration bench is sub-project 8).
 - **The flash is read-only.** On the first boot of a layout with OTA slots and a blank `otadata` partition, which is the layout of the Xteink X4, the bootloader records the slot it chose. inkemu reports the refused sector erase and page program (`flash command 0x20 is not modeled`, then `0x02`) and the boot goes on to the application; with `--strict-mmio` the run stops at the first of the two. Flash writes arrive with sub-project 1b-3b.
 - **Memory protection is not enforced.** The PMP entries, the permission registers and the stack guard are stored and never raise a fault (sub-project 1b-4).
 
@@ -151,7 +151,7 @@ Xteink devices are the first targets. The architecture is not tied to them, and
 
 ## Roadmap
 
-1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and a firmware on its own, is next.*
+1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and the device's flash file, is next; then 1b-3c, a firmware given on its own (`--firmware`); 1b-3d, the serial consoles in both directions; 1b-3e, a community firmware boots; 1b-3f, deep sleep; 1b-4, the lockstep against QEMU.*
 2. X4 peripherals, desktop UI, e-ink rendering: **first visual milestone**
 3. X3 support (UC8253 panel, I2C sensors)
 4. Debugging, resource monitor, headless test runner
@@ -175,11 +175,11 @@ Cette partie reprend le document en français, section par section et en plus co
 
 **inkemu est un émulateur au niveau binaire d'appareils embarqués à écran e-ink.** Il exécute l'image réelle d'un firmware, sans la modifier, sur une réplique virtuelle du matériel : on peut vérifier ce qu'un firmware affiche et comment il se comporte avant de le flasher.
 
-**État : début du développement.** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète, la ROM charge le bootloader de second niveau d'ESP-IDF, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail. Les écritures en flash et le démarrage d'un firmware fourni seul sont la prochaine étape (sous-projet 1b-3b), puis le lockstep avec QEMU (1b-4).
+**État : début du développement.** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète, la ROM charge le bootloader de second niveau d'ESP-IDF, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail. Les écritures en flash et le fichier de flash de l'appareil sont la prochaine étape (sous-projet 1b-3b) ; viennent ensuite le firmware fourni seul (1b-3c), les consoles série dans les deux sens (1b-3d), un firmware communautaire (1b-3e), le sommeil profond (1b-3f) et le lockstep avec QEMU (1b-4).
 
 ### Construire et essayer
 
-`cargo build --release`, puis `target/release/inkemu run --device x4 --flash flash.bin --max-cycles 50000000`, ou `--max-time 5` pour s'arrêter après cinq secondes d'appareil. `--flash` attend une **image flash complète** : le dump d'un appareil (`esptool.py read_flash 0 ALL flash.bin`) ou la sortie d'`esptool.py merge_bin`. Le fichier peut être plus court que la flash de l'appareil ; le reste se lit comme effacé. Un fichier qui ne peut pas être la flash de l'appareil est refusé avant toute exécution, avec la cause et le remède (code de sortie 2) : un `.xota` chiffré, une application seule, une image pour une autre puce, une image qui déclare plus de flash que l'appareil n'en a, un fichier plus long que la flash, un fichier vide. Sans `--flash`, la flash est effacée et la ROM affiche `invalid header: 0xffffffff` en boucle, comme une puce vierge. Il n'y a pas encore d'option pour démarrer un `.bin` d'application seul (sous-projet 1b-3b).
+`cargo build --release`, puis `target/release/inkemu run --device x4 --flash flash.bin --max-cycles 50000000`, ou `--max-time 5` pour s'arrêter après cinq secondes d'appareil. `--flash` attend une **image flash complète** : le dump d'un appareil (`esptool.py read_flash 0 ALL flash.bin`) ou la sortie d'`esptool.py merge_bin`. Le fichier peut être plus court que la flash de l'appareil ; le reste se lit comme effacé. Un fichier qui ne peut pas être la flash de l'appareil est refusé avant toute exécution, avec la cause et le remède (code de sortie 2) : un `.xota` chiffré, une application seule, une image pour une autre puce, une image qui déclare plus de flash que l'appareil n'en a, un fichier plus long que la flash, un fichier vide. Sans `--flash`, la flash est effacée et la ROM affiche `invalid header: 0xffffffff` en boucle, comme une puce vierge. Il n'y a pas encore d'option pour démarrer un `.bin` d'application seul (sous-projet 1b-3c).
 
 L'image de référence (`hello_world` d'ESP-IDF v5.5.2) vit toute sa vie : `Hello world!`, onze secondes de compte à rebours, `esp_restart()`, puis un second démarrage que la ROM annonce par `rst:0xc (RTC_SW_CPU_RST)`. Les secondes d'attente ne sont pas exécutées : le CPU dort dans `wfi` et le temps de l'appareil saute au prochain événement de timer, si bien que les onze secondes prennent environ une seconde de temps d'hôte. Avec `--strict-mmio`, la même exécution ne touche aucun registre qu'inkemu ne modélise pas et ne prend aucune exception, redémarrage compris.
 
@@ -188,9 +188,9 @@ L'image de référence (`hello_world` d'ESP-IDF v5.5.2) vit toute sa vie : `Hell
 - **Deux limites, deux horloges.** `--max-cycles N` arrête l'exécution quand le CPU a exécuté N cycles de son horloge ; `--max-time S` l'arrête quand S secondes du temps de l'appareil se sont écoulées depuis la mise sous tension (un nombre décimal : `30`, `2.5`, `0.001`). `--max-cycles` compte les cycles que le CPU exécute, non le temps de l'appareil : un CPU qui dort n'en exécute aucun alors que le temps de l'appareil continue, comme à travers les resets. Un firmware qui dort atteint donc cette limite tard (l'image de référence attend un timer pendant onze secondes et s'arrête tout de même à `--max-cycles 50000000`), et celui dont rien n'interrompt le sommeil ne l'atteint jamais ; `--max-time` borne les deux. Les deux peuvent être données ; la première atteinte termine l'exécution, avec le code de sortie 0. L'arrêt est exact et le même sur tout hôte : la première frontière d'instruction à la limite ou après, ou la limite elle-même quand le CPU dort. Un firmware qui attend une interruption que rien ne peut lever termine l'exécution de lui-même, avec le code 4.
 - **Ctrl-C et `kill`.** Sans limite, une exécution se termine par Ctrl-C, qui affiche le même résumé qu'une limite, avec le code 0. Un second Ctrl-C avant l'arrêt sort aussitôt, avec le code 130 et sans résumé. Un simple `kill` n'est pas géré et termine le processus sans résumé non plus. Avec `--trace-hash`, l'empreinte d'une exécution interrompue couvre ce qui a tourné jusqu'à la coupure, qui dépend de l'hôte : elle n'est pas reproductible ; celle d'une exécution terminée par une limite l'est.
 - **Les watchdogs sont réels.** Sans `--flash`, la ROM affiche `invalid header: 0xffffffff` jusqu'à l'expiration du watchdog que la puce arme au reset, 1,3 s plus tard, et redémarre avec `rst:0x7 (TG0WDT_SYS_RST)`, comme une puce vierge. Un firmware qui cesse de nourrir ses watchdogs est réinitialisé de la même façon, et le démarrage suivant nomme le watchdog et l'adresse où il a été pris (`Saved PC`). La ligne `reset by …` d'inkemu donne la cause, le compteur de programme et le pointeur de pile de chaque reset, y compris après un reset système, où la ROM n'affiche pas de `Saved PC`.
-- **La console est aussi lente que l'UART, et en sortie seulement.** Un octet part au débit que le firmware a programmé ; quand une exécution s'arrête, ce qui restait dans l'UART est affiché aussi. `--console` choisit quelle sortie série de l'appareil va sur la sortie standard : `uart0` (par défaut), `uart1` ou `usb-serial-jtag` ; la ROM écrit sur la première et sur la dernière. Rien ne peut encore être envoyé à l'appareil : ni réception UART, ni côté hôte de l'USB-Serial-JTAG (sous-projet 1b-3b).
+- **La console est aussi lente que l'UART, et en sortie seulement.** Un octet part au débit que le firmware a programmé ; quand une exécution s'arrête, ce qui restait dans l'UART est affiché aussi. `--console` choisit quelle sortie série de l'appareil va sur la sortie standard : `uart0` (par défaut), `uart1` ou `usb-serial-jtag` ; la ROM écrit sur la première et sur la dernière. Rien ne peut encore être envoyé à l'appareil : ni réception UART, ni côté hôte de l'USB-Serial-JTAG (sous-projet 1b-3d).
 - **Tubes et redirections.** `inkemu run … | head -5` se termine quand `head` a ses lignes : une exécution dont le lecteur est parti le dit et finit avec le code 0. Sous Unix, c'est vrai aussi quand le firmware n'affiche plus rien : de temps en temps, après une tranche qui n'avait rien à afficher, inkemu demande si son lecteur est encore là, et un lecteur parti est remarqué après 64 de ces tranches au plus (de quelques millisecondes à une demi-seconde sur la machine où cela a été mesuré, selon ce que fait le firmware). Une exécution qui finit d'elle-même entre-temps finit comme telle, avec le code de sortie de cette fin. Ailleurs, une telle exécution muette continue jusqu'à sa limite ou jusqu'à Ctrl-C. Quand la sortie standard ne peut pas être écrite pour une autre raison (un disque plein), l'exécution finit avec le code 2 ; un arrêt par `--strict-mmio` garde son code quoi qu'il soit arrivé à la console. Une sortie standard fermée (`>&-`) ne se distingue pas de `/dev/null`, parce que l'exécutable Rust la rouvre sur le périphérique nul avant que le programme ne commence : elle se comporte comme `/dev/null`, la console est abandonnée et l'exécution finit comme elle l'aurait fait, avec le code 0.
-- **Ce qui n'est pas encore émulé.** Le sommeil profond et le sommeil léger : un firmware qui demande l'un ou l'autre continue de tourner (sous-projet 1b-3b). Il n'y a encore ni radio, ni écran, ni bouton. La première ligne de l'erreur standard dit `timing model datasheet-uncalibrated (not calibrated)` : le coût en cycles de chaque instruction vient de documents, pas d'une mesure sur un appareil, donc un temps d'appareil est plausible, pas exact (le banc de calibration est le sous-projet 8).
+- **Ce qui n'est pas encore émulé.** Le sommeil profond et le sommeil léger : un firmware qui demande l'un ou l'autre continue de tourner. Le sous-projet 1b-3f modélise le sommeil profond et le réveil par le timer RTC, et refuse le sommeil léger en le nommant. Il n'y a encore ni radio, ni écran, ni bouton. La première ligne de l'erreur standard dit `timing model datasheet-uncalibrated (not calibrated)` : le coût en cycles de chaque instruction vient de documents, pas d'une mesure sur un appareil, donc un temps d'appareil est plausible, pas exact (le banc de calibration est le sous-projet 8).
 - **La flash est en lecture seule.** Au premier démarrage d'une disposition à emplacements OTA dont la partition `otadata` est vierge, celle du Xteink X4, le bootloader enregistre l'emplacement qu'il a choisi. inkemu signale l'effacement de secteur et la programmation de page refusés (`flash command 0x20 is not modeled`, puis `0x02`) et le démarrage continue jusqu'à l'application ; avec `--strict-mmio`, l'exécution s'arrête au premier des deux. Les écritures en flash arrivent avec le sous-projet 1b-3b.
 - **La protection mémoire n'est pas appliquée.** Les entrées PMP, les registres de permission et le garde-pile sont mémorisés et ne lèvent jamais de faute (sous-projet 1b-4).
 - **Codes de sortie** : 0 quand l'exécution s'est terminée normalement (limite de cycles ou limite en temps de l'appareil atteinte, Ctrl-C, ou lecteur de la sortie standard parti) ; 2 pour une erreur (appareil inconnu, ROM absente ou invalide, image flash refusée, sortie standard impossible à écrire) ; 3 pour un arrêt par `--strict-mmio` ; 4 quand le CPU attend une interruption et qu'aucune ne peut venir ; 130 pour un second Ctrl-C.
@@ -213,7 +213,7 @@ Espressif ESP32-C3 (RISC-V RV32IMC) : Xteink X4 et X3. Espressif ESP32-S3 (Xtens
 
 ### Feuille de route
 
-1. Cœur RV32IMC, ESP32-C3 minimal, chargeur de firmware, CLI : démarrer un firmware jusqu'à ses journaux de console. *1a fait ; 1b-1, squelette du SoC et démarrage de la ROM, fait ; 1b-2, flash et bootloader, fait ; 1b-3a, interruptions, timers, watchdogs et reset, fait ; 1b-3b, écritures en flash et firmware fourni seul, est le suivant.*
+1. Cœur RV32IMC, ESP32-C3 minimal, chargeur de firmware, CLI : démarrer un firmware jusqu'à ses journaux de console. *1a fait ; 1b-1, squelette du SoC et démarrage de la ROM, fait ; 1b-2, flash et bootloader, fait ; 1b-3a, interruptions, timers, watchdogs et reset, fait ; 1b-3b, écritures en flash et fichier de flash de l'appareil, est le suivant ; puis 1b-3c, firmware fourni seul (`--firmware`) ; 1b-3d, consoles série dans les deux sens ; 1b-3e, démarrage d'un firmware communautaire ; 1b-3f, sommeil profond ; 1b-4, lockstep avec QEMU.*
 2. Périphériques du X4, interface de bureau, rendu e-ink : **premier jalon visuel**
 3. Prise en charge du X3 (dalle UC8253, capteurs I2C)
 4. Débogage, moniteur de ressources, exécution de tests sans écran
````

- [ ] **Step 6: Check, and run the suite**

```bash
/usr/bin/git grep -n "SP1b-3b" -- crates > /tmp/t1-after.txt; wc -l < /tmp/t1-after.txt; cat /tmp/t1-after.txt
grep -c "| SP1b-3b |" docs/registers-accepted-without-effect.md
grep -c "| SP1b-3d |" docs/registers-accepted-without-effect.md
grep -c "| SP1b-3f |" docs/registers-accepted-without-effect.md
cargo test --workspace > /tmp/t1.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t1.log
```

Expected: `4` lines (three `"SP1b-3b",` rows of `periph/spi_mem.rs` and the comment of `tests/reference_boot.rs`), then `3`, `8`, `10`, then `exit=0` and `840` (the count of `main`: no test is added).

The check that proves the table: put `"SP1b-3b"` back as the heir of `RTC_CNTL.PWC` in `rtc_cntl.rs` and run `cargo test -p soc-esp32c3 --test accepted_without_effect`: `the_documented_table_is_the_one_the_code_builds` fails ("… is stale"). Undo it. (Task 14 adds the test that refuses a plan that is done as an heir.)

- [ ] **Step 7: Format, lint, commit**

```bash
/usr/bin/git add crates/soc-esp32c3/src/periph docs/registers-accepted-without-effect.md docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md README.md
/usr/bin/git commit -m "docs: what was handed to \"SP1b-3b\" is shared between five plans, each named

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src/periph docs/registers-accepted-without-effect.md docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md README.md
```

---
## Task 2: SPI1 hard-wired commands carry their address and data; one command per write of `CMD`

Until now a hard-wired command of SPI1 (`CMD` bits 31 to 19) sent its opcode and nothing else, and one write of `CMD` ran every bit that was set. The chip model refused every write, so nothing depended on it. This task makes the controller send what the ROM and ESP-IDF put in the registers: the address in `ADDR[23:0]`, for a page program its byte count in `ADDR[31:24]` and its bytes in `W0..`, for a status write `RD_STATUS`. It runs exactly one command per write of `CMD`. The chip still refuses the writes: Task 3 makes it take them.

**Files:**
- Modify: `crates/soc-esp32c3/src/flash.rs` (`FlashAddress`; `FlashCommand::address` becomes `Option<FlashAddress>`)
- Modify: `crates/soc-esp32c3/src/periph/spi_mem.rs` (the controller)
- Modify: `crates/soc-esp32c3/src/diag.rs` (three refusals)
- Modify: `crates/soc-esp32c3/src/bus.rs` (one test)
- Modify: `docs/registers-accepted-without-effect.md` (regenerated)

**Interfaces:**
- Consumes: `FlashDevice::execute(&mut self, &FlashCommand<'_>, &mut [u8]) -> Result<(), UnmodeledCommand>` (unchanged here), `Ctx::unsupported(Unsupported)`, `Storage`.
- Produces:
  - `pub struct FlashAddress { pub value: u32, pub bits: u32 }` in `soc_esp32c3::flash` (`Clone, Copy, Debug, PartialEq, Eq`); `FlashCommand::address: Option<FlashAddress>`.
  - `Unsupported::FlashProgramLength { addr: u32 }`, `Unsupported::FlashCommandBits { cmd: u32 }`, `Unsupported::FlashSuspend { sus_ctrl: u32 }`, with their messages and deduplication keys 15, 16, 17.
  - What a hard-wired command sends: `FLASH_PP` → opcode `02`, address `ADDR[23:0]` on 24 bits, `ADDR[31:24]` bytes of `W0..`; `FLASH_SE` → `20` and `FLASH_BE` → `D8`, with the address; `FLASH_CE` → `C7`, no address; `FLASH_WRSR` → `01` and one byte of `RD_STATUS`, or two when `CTRL` bit 22 (`WRSR_2B`) is set; `FLASH_READ` → `03` with the address, `MISO_DLEN` bytes into `W0..`.
  - `SPI1.FLASH_SUS_CTRL`: a write with bit 0, 1, 4 or 5 set is `Unsupported::FlashSuspend`; bits 1:0 read back 0.
  - The three `SPI1` rows of the table name `SP8`.

**Bounds on what the firmware writes.** The byte count of a program is one byte of `ADDR` (0 to 255): 0 and anything above `DATA_BYTES` (64, the 16 data registers) is refused before anything is sent, and `data_out(len)` takes at most those 64 bytes whatever `len` is. An address is masked to 24 bits. `CMD` is matched against 13 known bits; nothing iterates over it.

- [ ] **Step 1: Write the failing tests**

In `periph/spi_mem.rs` (module `tests`): the `Recorder` chip now records the address as a `FlashAddress`, and the tests of the hard-wired commands are added; the test `an_esp_idf_program_sets_the_flag_with_usr_and_its_opcode_runs_once` is renamed, because `USR | FLASH_PE` is not how ESP-IDF programs (the measured program is `CMD = 0x02000000`, `ADDR = len << 24 | addr`: SF §B.1).

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -513,6 +643,7 @@ mod tests {
     fn program_and_erase_are_reported_by_opcode() {
         let ctx = &mut Ctx::default();
         let mut spi = spi1();
+        spi.write(ctx, ADDR, 0x0400_E000); // a program of 4 bytes
         for (command_bit, opcode) in [
             (1u32 << 25, 0x02u8), // PP
             (1 << 24, 0x20),      // SE
@@ -584,9 +715,15 @@ mod tests {
     }
 
     /// What a flash saw: `(opcode, address, dummy cycles, bytes sent)`.
-    type Seen = (u8, Option<u32>, u32, Vec<u8>);
+    type Seen = (u8, Option<FlashAddress>, u32, Vec<u8>);
+
+    /// A 24-bit address, the width of every hard-wired command.
+    const fn at(value: u32) -> FlashAddress {
+        FlashAddress { value, bits: 24 }
+    }
 
     /// A chip that accepts every command and remembers it, to look at what the controller sends.
+    /// It answers `A0 A1 A2 ...` to whatever is read.
     #[derive(Debug)]
     struct Recorder(Rc<RefCell<Vec<Seen>>>);
 
@@ -600,8 +737,11 @@ mod tests {
         fn execute(
             &mut self,
             command: &FlashCommand<'_>,
-            _data_in: &mut [u8],
+            data_in: &mut [u8],
         ) -> Result<(), UnmodeledCommand> {
+            for (byte, answer) in data_in.iter_mut().zip(0xA0u8..) {
+                *byte = answer;
+            }
             self.0.borrow_mut().push((
                 command.opcode,
                 command.address,
@@ -618,32 +758,190 @@ mod tests {
     }
 
     #[test]
-    fn several_refused_commands_in_one_write_are_all_reported_in_the_order_they_ran() {
+    fn one_write_of_cmd_with_several_command_bits_runs_none_and_says_so() {
         let ctx = &mut Ctx::default();
-        let mut spi = spi1();
-        assert!(spi.write(ctx, CMD, (1 << 25) | (1 << 24))); // PP and SE
-        assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0x02 })
-        );
-        assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0x20 })
-        );
-        assert_eq!(ctx.take_unsupported(), None, "drained");
-        // A refused hard-wired command and a refused user command in the same write.
+        let (mut spi, seen) = recording_spi1();
+        spi.write(ctx, ADDR, 0x0400_9000);
         spi.write(ctx, USER, USR_COMMAND);
-        spi.write(ctx, USER2, 0x7000_00D7);
-        spi.write(ctx, CMD, (1 << 23) | CMD_USR);
+        spi.write(ctx, USER2, 0x7000_0006);
+        for cmd in [
+            CMD_PP | CMD_SE,
+            CMD_BE | CMD_USR,
+            COMMAND_BITS,
+            (1 << 30) | (1 << 29) | CMD_FLASH_PE,
+        ] {
+            assert!(spi.write(ctx, CMD, cmd), "handled: reported, not dropped");
+            assert_eq!(
+                ctx.take_unsupported(),
+                Some(Unsupported::FlashCommandBits { cmd })
+            );
+            assert_eq!(ctx.take_unsupported(), None, "one write, one report");
+            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
+        }
+        assert_eq!(seen.borrow().as_slice(), [], "nothing reached the chip");
+        // The flag and the read-only state bits [7:0] are not commands: beside one command bit
+        // they change nothing.
+        assert!(spi.write(ctx, CMD, CMD_SE | CMD_FLASH_PE | 0xFF));
+        assert_eq!(seen.borrow().len(), 1);
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn a_hard_wired_program_sends_its_address_and_the_bytes_its_count_names() {
+        // The ROM's `otadata` program (FW §A.3: `ADDR = 0x2000E000`, `W0..W7`, `CMD = 0x02000000`)
+        // and ESP-IDF's (SF §B.2: `0x04009020` is 4 bytes at 0x9020; 64 bytes at most, FW §A.2).
+        let ctx = &mut Ctx::default();
+        let (mut spi, seen) = recording_spi1();
+        for word in 0..DATA_WORDS {
+            spi.write(ctx, W0 + 4 * word, 0x0403_0201 + 0x0404_0404 * word);
+        }
+        for (addr, address, len) in [
+            (0x2000_E000u32, 0xE000u32, 32usize),
+            (0x0400_9020, 0x9020, 4),
+            (0x40FF_FF00, 0xFF_FF00, 64),
+            (0x0100_0000, 0, 1),
+        ] {
+            spi.write(ctx, ADDR, addr);
+            assert!(spi.write(ctx, CMD, CMD_PP));
+            let last = seen.borrow().last().cloned();
+            let bytes: Vec<u8> = (1..=64u8).take(len).collect();
+            assert_eq!(
+                last,
+                Some((0x02, Some(at(address)), 0, bytes)),
+                "ADDR {addr:#010x}"
+            );
+        }
+        assert_eq!(seen.borrow().len(), 4, "one write of CMD, one transfer");
+        assert_eq!(ctx.take_unsupported(), None);
+        assert_eq!(spi.read(ctx, CMD), Some(0));
+    }
+
+    #[test]
+    fn a_hard_wired_program_of_no_byte_or_of_more_than_64_is_refused_and_sends_nothing() {
+        let ctx = &mut Ctx::default();
+        let (mut spi, seen) = recording_spi1();
+        for addr in [0x0000_9000u32, 0x4100_9000, 0xFF00_9000] {
+            spi.write(ctx, ADDR, addr);
+            assert!(spi.write(ctx, CMD, CMD_PP));
+            assert_eq!(
+                ctx.take_unsupported(),
+                Some(Unsupported::FlashProgramLength { addr }),
+                "ADDR {addr:#010x}"
+            );
+            assert_eq!(ctx.take_unsupported(), None);
+            assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears: no hang");
+        }
+        assert_eq!(seen.borrow().as_slice(), [], "nothing reached the chip");
+    }
+
+    #[test]
+    fn sector_and_block_erase_send_the_low_24_bits_of_addr_with_or_without_the_flag() {
+        // ROM: `CMD = 0x01020000` and `0x00820000` (FW §A.3); ESP-IDF: `0x01000000` and
+        // `0x00800000` (FW §A.2, SF §D).
+        let ctx = &mut Ctx::default();
+        for (cmd, opcode) in [
+            (0x0102_0000u32, 0x20u8),
+            (0x0100_0000, 0x20),
+            (0x0082_0000, 0xD8),
+            (0x0080_0000, 0xD8),
+        ] {
+            let (mut spi, seen) = recording_spi1();
+            // Bits 31:24 belong to the byte count of a program, not to an erase address.
+            spi.write(ctx, ADDR, 0xAB00_E000);
+            assert!(spi.write(ctx, CMD, cmd));
+            assert_eq!(
+                seen.borrow().as_slice(),
+                [(opcode, Some(at(0xE000)), 0, Vec::new())],
+                "CMD {cmd:#010x}"
+            );
+            assert_eq!(ctx.take_unsupported(), None);
+            assert_eq!(spi.read(ctx, CMD), Some(0));
+        }
+    }
+
+    #[test]
+    fn chip_erase_and_the_latch_commands_send_an_opcode_and_nothing_else() {
+        let ctx = &mut Ctx::default();
+        let (mut spi, seen) = recording_spi1();
+        spi.write(ctx, ADDR, 0x2000_E000);
+        for (cmd, opcode) in [(1u32 << 22, 0xC7u8), (1 << 30, 0x06), (1 << 29, 0x04)] {
+            spi.write(ctx, CMD, cmd);
+            let last = seen.borrow().last().cloned();
+            assert_eq!(last, Some((opcode, None, 0, Vec::new())), "CMD {cmd:#010x}");
+        }
+    }
+
+    #[test]
+    fn the_status_write_sends_one_byte_of_rd_status_or_two_when_ctrl_says_so() {
+        // `esp_rom_spiflash_write_status`: `RD_STATUS[15:0]` = the status, `CMD = 0x04000000`.
+        let ctx = &mut Ctx::default();
+        let (mut spi, seen) = recording_spi1();
+        spi.write(ctx, RD_STATUS, 0xFFFF_3C9C);
+        spi.write(ctx, CMD, CMD_WRSR);
+        let ctrl = spi.register(CTRL);
+        spi.write(ctx, CTRL, ctrl | CTRL_WRSR_2B);
+        spi.write(ctx, CMD, CMD_WRSR);
         assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0xD8 })
+            seen.borrow().as_slice(),
+            [
+                (0x01, None, 0, vec![0x9C]),
+                (0x01, None, 0, vec![0x9C, 0x3C])
+            ]
         );
+    }
+
+    #[test]
+    fn the_hard_wired_read_sends_its_address_and_stores_what_the_chip_answers() {
+        // Pins the ASSUMPTION documented in `Spi1::run_hard_wired`.
+        let ctx = &mut Ctx::default();
+        let (mut spi, seen) = recording_spi1();
+        spi.write(ctx, ADDR, 0x7712_3456);
+        spi.write(ctx, MISO_DLEN, 8 * 6 - 1);
+        spi.write(ctx, W0 + 4, 0xFFFF_FFFF);
+        assert!(spi.write(ctx, CMD, CMD_READ));
         assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0xD7 })
+            seen.borrow().as_slice(),
+            [(0x03, Some(at(0x12_3456)), 0, Vec::new())]
         );
+        assert_eq!(spi.read(ctx, W0), Some(0xA3A2_A1A0));
+        assert_eq!(spi.read(ctx, W0 + 4), Some(0x0000_A5A4));
+        assert_eq!(spi.read(ctx, CMD), Some(0));
+    }
+
+    #[test]
+    fn a_suspend_or_resume_request_is_refused_by_name_and_the_measured_configuration_is_stored() {
+        let ctx = &mut Ctx::default();
+        let (mut spi, seen) = recording_spi1();
+        // What ESP-IDF writes at every bus acquisition (FW §A.2): no enable bit, a time-out count.
+        assert!(spi.write(ctx, FLASH_SUS_CTRL, 0x0A00_0000));
+        assert_eq!(spi.read(ctx, FLASH_SUS_CTRL), Some(0x0A00_0000));
+        // The two wait enables ask for nothing by themselves.
+        assert!(spi.write(ctx, FLASH_SUS_CTRL, 0b1100));
         assert_eq!(ctx.take_unsupported(), None);
+        for (value, stored) in [
+            (1u32 << 5, 1u32 << 5), // FLASH_PES_EN: auto-suspend
+            (1 << 4, 1 << 4),       // PES_PER_EN: auto-resume
+            (0b10, 0),              // FLASH_PES: send a suspend
+            (0b01, 0),              // FLASH_PER: send a resume
+            (0x0A00_0023, 0x0A00_0020),
+        ] {
+            assert!(spi.write(ctx, FLASH_SUS_CTRL, value));
+            assert_eq!(
+                ctx.take_unsupported(),
+                Some(Unsupported::FlashSuspend { sus_ctrl: value }),
+                "{value:#x}"
+            );
+            assert_eq!(
+                spi.read(ctx, FLASH_SUS_CTRL),
+                Some(stored),
+                "the two command bits clear themselves"
+            );
+        }
+        assert_eq!(
+            seen.borrow().as_slice(),
+            [],
+            "no suspend or resume opcode is sent"
+        );
     }
 
     #[test]
@@ -676,20 +974,21 @@ mod tests {
         spi.write(ctx, USER, USR_COMMAND | USR_ADDR);
         spi.write(ctx, USER2, 0x7000_005A);
         spi.write(ctx, ADDR, 0xDEAD_BEEF);
-        // (field value, address the flash must see): the field is the bit length minus one.
-        for (field, expected) in [
-            (0, 0x1),
-            (7, 0xEF),
-            (23, 0xAD_BEEF),
-            (31, 0xDEAD_BEEF),
-            (63, 0xDEAD_BEEF), // 64 bits asked, the register holds 32
+        // (field value, address the flash must see, on how many bits): the field is the bit
+        // length minus one.
+        for (field, value, bits) in [
+            (0, 0x1, 1),
+            (7, 0xEF, 8),
+            (23, 0xAD_BEEF, 24),
+            (31, 0xDEAD_BEEF, 32),
+            (63, 0xDEAD_BEEF, 32), // 64 bits asked, the register holds 32
         ] {
             spi.write(ctx, USER1, field << 26);
             spi.write(ctx, CMD, CMD_USR);
             let last = seen.borrow().last().cloned();
             assert_eq!(
                 last.map(|s| s.1),
-                Some(Some(expected)),
+                Some(Some(FlashAddress { value, bits })),
                 "address field {field}"
             );
         }
@@ -762,8 +1061,12 @@ mod tests {
         assert_eq!(ROM_SECTOR_ERASE, (1 << 24) | CMD_FLASH_PE);
         let ctx = &mut Ctx::default();
         let (mut spi, seen) = recording_spi1();
+        spi.write(ctx, ADDR, 0xE000);
         assert!(spi.write(ctx, CMD, ROM_SECTOR_ERASE));
-        assert_eq!(seen.borrow().as_slice(), [(0x20, None, 0, Vec::new())]);
+        assert_eq!(
+            seen.borrow().as_slice(),
+            [(0x20, Some(at(0xE000)), 0, Vec::new())]
+        );
         assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears");
         // On the read-only chip the erase is refused once, by its own opcode: `USER2` (0x00 in its low
         // byte at reset) is not a second command.
@@ -777,8 +1080,12 @@ mod tests {
     }
 
     #[test]
-    fn an_esp_idf_program_sets_the_flag_with_usr_and_its_opcode_runs_once() {
+    fn a_user_transfer_that_carries_the_program_erase_flag_runs_once() {
         // `spimem_flash_ll_user_start(dev, pe_ops = true)` ORs 0x60000 into CMD: `USR | FLASH_PE`.
+        // ESP-IDF v5.5.2 does it for the program and erase commands that take a 4-byte address,
+        // above 16 MiB (`spi_flash_chip_generic.c:118-151`); below, its program is the hard-wired
+        // `CMD = 0x02000000` (SF key result 12). What is pinned here is the flag beside `USR`,
+        // with a transfer the chip of this test understands: a 3-byte-address program.
         const USER_START_PE: u32 = 0x0006_0000;
         assert_eq!(USER_START_PE, CMD_USR | CMD_FLASH_PE);
         let ctx = &mut Ctx::default();
@@ -795,7 +1102,7 @@ mod tests {
         page_program(&mut spi);
         assert_eq!(
             seen.borrow().as_slice(),
-            [(0x02, Some(0xE000), 0, vec![1, 2, 3, 4])],
+            [(0x02, Some(at(0xE000)), 0, vec![1, 2, 3, 4])],
             "one transfer, with its address and its data"
         );
         let mut refused = spi1();
@@ -860,8 +1167,8 @@ mod tests {
         for name in ["MISC", "FLASH_SUS_CTRL", "FLASH_WAITI_CTRL"] {
             let offset = svd_regs::esp32c3::addr("SPI1", name) - SPI1_BASE;
             assert_eq!(spi1.read(ctx, offset), Some(reset("SPI1", name)), "{name}");
-            assert!(spi1.write(ctx, offset, 0x5), "{name}");
-            assert_eq!(spi1.read(ctx, offset), Some(0x5), "{name}");
+            assert!(spi1.write(ctx, offset, 0x0A00_0000), "{name}");
+            assert_eq!(spi1.read(ctx, offset), Some(0x0A00_0000), "{name}");
         }
         let mut spi0 = Spi0::new();
         for name in ["CORE_CLK_SEL", "MISC"] {
````

In `flash.rs`, `diag.rs` and `bus.rs` (their `tests` modules):

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -489,7 +498,10 @@ mod tests {
         let mut byte = [0u8];
         let command = FlashCommand {
             opcode: 0x5A,
-            address: Some(0x10),
+            address: Some(FlashAddress {
+                value: 0x10,
+                bits: 24,
+            }),
             dummy_cycles: 8,
             data_out: &[],
         };
````

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -566,6 +591,9 @@ mod tests {
                 uart: 1,
                 sclk_sel: 2,
             }),
+            unsupported(Unsupported::FlashProgramLength { addr: 0x4100_9000 }),
+            unsupported(Unsupported::FlashCommandBits { cmd: 0x0300_0000 }),
+            unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
             mapping(3, MappingFault::InvalidEntry),
             mapping(3, MappingFault::BeyondFlash),
         ] {
@@ -732,6 +760,18 @@ mod tests {
                 }),
                 "UART1 sent a byte with a source clock inkemu does not model (CLK_CONF.SCLK_SEL = 2",
             ),
+            (
+                unsupported(Unsupported::FlashProgramLength { addr: 0x4100_9000 }),
+                "program 65 bytes with one FLASH_PP command (ADDR 0x41009000",
+            ),
+            (
+                unsupported(Unsupported::FlashCommandBits { cmd: 0x0300_0000 }),
+                "several command bits at once (value 0x03000000",
+            ),
+            (
+                unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
+                "program/erase suspend or resume (value 0x00000020",
+            ),
             (mapping(3, MappingFault::InvalidEntry), "MMU entry 3"),
             (
                 mapping(3, MappingFault::BeyondFlash),
````

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -766,7 +766,7 @@ mod tests {
     }
 
     #[test]
-    fn every_refused_flash_command_of_one_write_is_reported_and_the_first_stops_a_strict_run() {
+    fn a_write_of_several_flash_command_bits_is_one_named_diagnostic_and_a_strict_violation() {
         let cmd = svd_regs::esp32c3::addr("SPI1", "CMD");
         let mut b = bus_with(SocConfig {
             strict_mmio: true,
@@ -774,12 +774,12 @@ mod tests {
         });
         b.set_pc(0x4038_0020);
         b.write(cmd, Word, (1 << 25) | (1 << 24)).unwrap(); // page program and sector erase
-        let refused = |opcode| Diagnostic::Unsupported {
-            what: crate::diag::Unsupported::FlashCommand { opcode },
+        let refused = Diagnostic::Unsupported {
+            what: crate::diag::Unsupported::FlashCommandBits { cmd: 0x0300_0000 },
             pc: 0x4038_0020,
         };
-        assert_eq!(b.ctx.diag.drain(), vec![refused(0x02), refused(0x20)]);
-        assert_eq!(b.ctx.diag.take_violation(), Some(refused(0x02)));
+        assert_eq!(b.ctx.diag.drain(), vec![refused.clone()]);
+        assert_eq!(b.ctx.diag.take_violation(), Some(refused));
     }
 
     #[test]
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib > /tmp/t2-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t2-red.log
```

Expected: `exit=101`; the crate does not compile: ``error[E0422]: cannot find struct, variant or union type `FlashAddress` in this scope``, then `CMD_PP`, `CMD_SE`, `CMD_BE`, `COMMAND_BITS` not found.

- [ ] **Step 3: The address of a transaction has a width**

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -171,11 +171,20 @@ pub enum FlashImageError {
     TooLong { len: usize, size: u32 },
 }
 
+/// The address phase of a transaction: the value the controller clocks out after the opcode, and
+/// on how many bits.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub struct FlashAddress {
+    pub value: u32,
+    pub bits: u32,
+}
+
 /// One SPI transaction as the chip sees it.
 #[derive(Clone, Copy, Debug)]
 pub struct FlashCommand<'a> {
     pub opcode: u8,
-    pub address: Option<u32>,
+    /// `None` when the transaction has no address phase.
+    pub address: Option<FlashAddress>,
     pub dummy_cycles: u32,
     /// Bytes the controller sends after the address and the dummy cycles.
     pub data_out: &'a [u8],
````

- [ ] **Step 4: The three refusals**

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -28,6 +28,14 @@ pub enum Unsupported {
         user: u32,
         user2: u32,
     },
+    /// A hard-wired page program (`SPI1.CMD` bit `FLASH_PP`) whose byte count, `ADDR[31:24]`, is 0
+    /// or more than the 64 bytes `W0..W15` hold; `addr` is the whole register.
+    FlashProgramLength { addr: u32 },
+    /// One write of `SPI1.CMD` with more than one command bit set; `cmd` is the value written.
+    FlashCommandBits { cmd: u32 },
+    /// A write of `SPI1.FLASH_SUS_CTRL` that sends a suspend or a resume, or enables the automatic
+    /// ones; `sus_ctrl` is the value written.
+    FlashSuspend { sus_ctrl: u32 },
     /// A write to `SHA.DMA_BLOCK_NUM`, `DMA_START` or `DMA_CONTINUE` (D7).
     ShaDma,
     /// A `SHA.MODE` the ESP32-C3 accelerator does not have.
@@ -71,6 +79,19 @@ impl Unsupported {
                     " (USER {user:#010x}, USER2 {user2:#010x}, pc {pc:#010x}) but only transfers with an 8-bit command are modeled, so nothing was sent to the flash; use firmware whose flash commands are 8 bits long until other transfers are emulated"
                 )
             }
+            Unsupported::FlashProgramLength { addr } => write!(
+                f,
+                "SPI1 was asked to program {len} bytes with one FLASH_PP command (ADDR {addr:#010x}, pc {pc:#010x}) but one command carries 1 to 64 bytes, what W0..W15 hold, so nothing was sent to the flash; put the byte count, 1 to 64, in ADDR[31:24]",
+                len = addr >> 24
+            ),
+            Unsupported::FlashCommandBits { cmd } => write!(
+                f,
+                "SPI1.CMD was written with several command bits at once (value {cmd:#010x}, pc {pc:#010x}) and the order in which the controller would run them is not documented, so none was run; start one flash command per write of SPI1.CMD"
+            ),
+            Unsupported::FlashSuspend { sus_ctrl } => write!(
+                f,
+                "SPI1.FLASH_SUS_CTRL asks for a program/erase suspend or resume (value {sus_ctrl:#010x}, pc {pc:#010x}) but a flash operation completes at once here and is never suspended, so nothing was sent to the flash; keep flash auto-suspend off (CONFIG_SPI_FLASH_AUTO_SUSPEND=n, the ESP-IDF default)"
+            ),
             Unsupported::ShaDma => write!(
                 f,
                 "SHA.DMA_BLOCK_NUM, DMA_START or DMA_CONTINUE was written (pc {pc:#010x}) but only the typical SHA mode is modeled, so DMA-SHA produces no digest; use firmware that hashes through SHA.START/CONTINUE until DMA-SHA is emulated"
@@ -197,6 +218,10 @@ impl Diagnostic {
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
                 }
+                // One line per byte count, per set of command bits, and one for suspend requests.
+                Unsupported::FlashProgramLength { addr } => (15, addr >> 24, 0),
+                Unsupported::FlashCommandBits { cmd } => (16, cmd, 0),
+                Unsupported::FlashSuspend { .. } => (17, 0, 0),
             },
             Diagnostic::FlashMapping { entry, fault, .. } => (10, entry, fault as u32),
             Diagnostic::Reset { cause, pc, .. } => (14, pc, cause.code()),
````

- [ ] **Step 5: The controller**

`CMD` bits 1 and 0 were listed as suspend and resume commands (`75`, `7A`): they are read-only state fields in the SVD, and the two commands live in `FLASH_SUS_CTRL`. They leave the table of hard-wired commands.

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -4,12 +4,14 @@
 use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
 use crate::ctx::Ctx;
 use crate::diag::Unsupported;
-use crate::flash::{FlashCommand, FlashDevice};
+use crate::flash::{FlashAddress, FlashCommand, FlashDevice};
 use emu_core::Peripheral;
 use svd_regs::esp32c3::addr;
 
 const CMD: u32 = addr("SPI1", "CMD") - SPI1_BASE;
 const ADDR: u32 = addr("SPI1", "ADDR") - SPI1_BASE;
+const CTRL: u32 = addr("SPI1", "CTRL") - SPI1_BASE;
+const FLASH_SUS_CTRL: u32 = addr("SPI1", "FLASH_SUS_CTRL") - SPI1_BASE;
 const USER: u32 = addr("SPI1", "USER") - SPI1_BASE;
 const USER1: u32 = addr("SPI1", "USER1") - SPI1_BASE;
 const USER2: u32 = addr("SPI1", "USER2") - SPI1_BASE;
@@ -24,29 +26,58 @@ const WORD_BYTES: usize = 4;
 
 /// `CMD` bit 18: run the transfer described by `USER`, `USER1`, `USER2`, `ADDR` and the lengths.
 const CMD_USR: u32 = 1 << 18;
+const CMD_READ: u32 = 1 << 31;
 const CMD_RDID: u32 = 1 << 28;
 const CMD_RDSR: u32 = 1 << 27;
-/// The hard-wired commands: `CMD` bit, the opcode the controller sends (ESP-IDF `spi_flash_defs.h`,
-/// report BC §D.2 item 6; `0xB9` deep power-down is the JEDEC opcode, it is not in the report).
-const HARD_WIRED: [(u32, u8); 15] = [
-    (1 << 31, 0x03), // FLASH_READ
+const CMD_WRSR: u32 = 1 << 26;
+const CMD_PP: u32 = 1 << 25;
+const CMD_SE: u32 = 1 << 24;
+const CMD_BE: u32 = 1 << 23;
+/// The hard-wired commands: `CMD` bit, the opcode the controller sends (report FW §A.1; ESP-IDF
+/// `spi_flash_defs.h`; `0xB9` deep power-down is the JEDEC opcode). `FLASH_BE` is the 64 KiB
+/// erase `D8`, whatever the header's "32KB" says: the ROM and ESP-IDF both use it for 64 KiB.
+///
+/// `CMD` [7:0] are two read-only state fields (`SPI1_MST_ST`, `MSPI_ST`): no command lives
+/// there. Suspend and resume are bits 1 and 0 of `FLASH_SUS_CTRL`.
+const HARD_WIRED: [(u32, u8); 13] = [
+    (CMD_READ, 0x03),
     (1 << 30, 0x06), // WREN
     (1 << 29, 0x04), // WRDI
     (CMD_RDID, 0x9F),
     (CMD_RDSR, 0x05),
-    (1 << 26, 0x01), // WRSR
-    (1 << 25, 0x02), // PP
-    (1 << 24, 0x20), // SE
-    (1 << 23, 0xD8), // BE
+    (CMD_WRSR, 0x01),
+    (CMD_PP, 0x02),
+    (CMD_SE, 0x20),
+    (CMD_BE, 0xD8),
     (1 << 22, 0xC7), // CE
     (1 << 21, 0xB9), // DP
     (1 << 20, 0xAB), // RES
     (1 << 19, 0xA3), // HPM
-    (1 << 1, 0x75),  // PES
-    (1 << 0, 0x7A),  // PER
 ];
+/// Every bit of `CMD` that starts a transfer: the hard-wired commands, bits 31 to 19, and `USR`.
+/// Bit 17, `FLASH_PE`, is below them: it is a flag (see [`Spi1::run`]).
+const COMMAND_BITS: u32 = !0 << 18;
 const JEDEC_ID_BYTES: usize = 3;
 const STATUS_BYTE_MASK: u32 = 0xFF;
+/// A hard-wired command sends the low 24 bits of `ADDR`: `addr & 0xffffff` for an erase, the
+/// same under the byte count for a program (`ROM3:SPI_sector_erase@0x4004d294`,
+/// `ROM3:SPI_page_program@0x4004d6b4`; `IDF:hal/esp32c3/include/hal/spimem_flash_ll.h:81-95,
+/// 377-389`; measured: report FW §A.2, §A.3).
+const HARD_WIRED_ADDRESS_BITS: u32 = 24;
+const HARD_WIRED_ADDRESS_MASK: u32 = 0x00FF_FFFF;
+/// `ADDR` [31:24] of a hard-wired page program: the number of bytes (`0x20009000` is 32 bytes at
+/// `0x9000`; reports FW key result 1, SF §B.2).
+const PP_LENGTH_SHIFT: u32 = 24;
+/// `CTRL` bit 22, `WRSR_2B`: the status write sends two bytes instead of one
+/// (`spi_mem_reg.h:158-161`). `ROM3:Enable_QMode` sets it (`CTRL |= 0x400000` at `0x4004d792` to
+/// `0x4004d7a0`) before it writes a 16-bit status with `esp_rom_spiflash_write_status`.
+const CTRL_WRSR_2B: u32 = 1 << 22;
+/// `FLASH_SUS_CTRL`: bit 0 `FLASH_PER` and bit 1 `FLASH_PES` send a resume and a suspend and clear
+/// themselves (`spimem_flash_ll_resume`, `spimem_flash_ll_suspend`); bit 4 `PES_PER_EN` and bit 5
+/// `FLASH_PES_EN` enable the automatic ones (`spimem_flash_ll_auto_resume_init`,
+/// `spimem_flash_ll_auto_suspend_init`: `spimem_flash_ll.h:102-139`, `spi_mem_reg.h:821-856`).
+const SUS_COMMANDS: u32 = 0b11;
+const SUS_REQUESTS: u32 = SUS_COMMANDS | (1 << 4) | (1 << 5);
 
 /// `USER` bit 31: the transfer has a command phase.
 const USR_COMMAND: u32 = 1 << 31;
@@ -70,10 +101,17 @@ const MODELED_COMMAND_BITS: u32 = 8;
 const DLEN_MASK: u32 = 0x3FF;
 
 /// Configuration of SPI1 that nothing acts on here: what shapes the electrical transfer, which
-/// is instantaneous, and the automatic wait, suspend and resume around a program or an erase,
-/// which arrive with the flash writes.
+/// is instantaneous, and the automatic wait, suspend and resume around a program or an erase.
+/// A flash operation completes at once (report FW §A.5: every measured poll ends on its first
+/// read), so there is nothing to wait for and nothing to suspend: these three rows wait for the
+/// plan that gives a flash operation its duration.
 const SPI1_ACCEPTED: [Accepted; 9] = [
-    Accepted::svd("SPI1", "CTRL", "read mode, dummy output, WP level", "SP8"),
+    Accepted::svd(
+        "SPI1",
+        "CTRL",
+        "read mode, dummy output, WP level (the two-byte status write bit acts)",
+        "SP8",
+    ),
     Accepted::svd(
         "SPI1",
         "CTRL1",
@@ -90,8 +128,8 @@ const SPI1_ACCEPTED: [Accepted; 9] = [
     Accepted::svd(
         "SPI1",
         "SUS_STATUS",
-        "program/erase suspend state",
-        "SP1b-3b",
+        "program/erase suspend state: nothing is ever suspended",
+        "SP8",
     ),
     Accepted::svd("SPI1", "CLOCK_GATE", "register clock gate", "SP8"),
     // `spi_flash_hal_device_config` and what it calls (`IDF:hal/spi_flash_hal_common.inc:45`; F1).
@@ -99,14 +137,14 @@ const SPI1_ACCEPTED: [Accepted; 9] = [
     Accepted::svd(
         "SPI1",
         "FLASH_WAITI_CTRL",
-        "automatic wait for the chip to be idle after a program or an erase",
-        "SP1b-3b",
+        "automatic wait for the chip to be idle after a program or an erase: an operation completes at once",
+        "SP8",
     ),
     Accepted::svd(
         "SPI1",
         "FLASH_SUS_CTRL",
-        "automatic program/erase suspend and resume",
-        "SP1b-3b",
+        "program/erase suspend and resume: an operation completes at once (a suspend or resume request is refused by name)",
+        "SP8",
     ),
 ];
 
@@ -241,7 +279,23 @@ impl Spi1 {
         (self.register(USER2) & OPCODE_MASK).to_le_bytes()[0]
     }
 
+    /// The first `len` bytes of `W0..W15`, the first byte in `W0[7:0]`; at most the 64 there are.
+    fn data_out(&self, len: usize) -> Vec<u8> {
+        (0..DATA_WORDS)
+            .flat_map(|word| self.register(W0 + 4 * word).to_le_bytes())
+            .take(len)
+            .collect()
+    }
+
+    /// Runs the hard-wired command of `command_bit`. The ROM and ESP-IDF write the same registers
+    /// for the same command (reports FW §A.2, §A.3; SF §B.1, §D): the address in `ADDR[23:0]`, for a
+    /// program its byte count in `ADDR[31:24]` and its bytes in `W0..`.
     fn run_hard_wired(&mut self, ctx: &mut Ctx, command_bit: u32, opcode: u8) {
+        let addr = self.register(ADDR);
+        let address = Some(FlashAddress {
+            value: addr & HARD_WIRED_ADDRESS_MASK,
+            bits: HARD_WIRED_ADDRESS_BITS,
+        });
         let command = FlashCommand {
             opcode,
             address: None,
@@ -263,6 +317,53 @@ impl Spi1 {
                     self.registers.write(RD_STATUS, kept | low);
                 }
             }
+            CMD_PP => {
+                // The byte count is one byte of a register the firmware writes: 0 to 255. The
+                // buffer holds 64 bytes, and neither the ROM (32 at most) nor ESP-IDF (64 at
+                // most) asks for more: what the controller would send for another count is not
+                // established, so such a program is refused rather than guessed (report FW §G).
+                let len = usize::from((addr >> PP_LENGTH_SHIFT).to_le_bytes()[0]);
+                if len == 0 || len > DATA_BYTES {
+                    ctx.unsupported(Unsupported::FlashProgramLength { addr });
+                    return;
+                }
+                let data = self.data_out(len);
+                let program = FlashCommand {
+                    address,
+                    data_out: &data,
+                    ..command
+                };
+                let _ = self.transfer(ctx, &program, 0);
+            }
+            CMD_SE | CMD_BE => {
+                let _ = self.transfer(ctx, &FlashCommand { address, ..command }, 0);
+            }
+            CMD_WRSR => {
+                // The value is `RD_STATUS[15:0]`, where `ROM3:esp_rom_spiflash_write_status`
+                // puts it (`0x4004d35e` to `0x4004d370`) before it writes `CMD = 0x04000000`.
+                let status = self.register(RD_STATUS).to_le_bytes();
+                let bytes = if self.register(CTRL) & CTRL_WRSR_2B == 0 {
+                    1
+                } else {
+                    2
+                };
+                let write = FlashCommand {
+                    data_out: &status[..bytes],
+                    ..command
+                };
+                let _ = self.transfer(ctx, &write, 0);
+            }
+            CMD_READ => {
+                // ASSUMPTION: the hard-wired read sends `ADDR[23:0]` as the erase commands do
+                // and clocks in the bytes `MISO_DLEN` asks for, into `W0..`. `spi_mem_reg.h:18`
+                // names the bit and nothing more, and no ROM or ESP-IDF v5.5.2 code writes it
+                // (report FW §A.1: "nobody"; the ROM reads through `USR`). A firmware that uses
+                // it, read against a device, settles the encoding.
+                let len = self.data_len(MISO_DLEN);
+                if let Some(data) = self.transfer(ctx, &FlashCommand { address, ..command }, len) {
+                    self.store_data_in(&data);
+                }
+            }
             _ => {
                 let _ = self.transfer(ctx, &command, 0);
             }
@@ -310,7 +411,11 @@ impl Spi1 {
             // ASSUMPTION: the address is the low `bits` bits of ADDR, unshifted (ESP-IDF
             // `spimem_flash_ll_set_usr_address` stores it as is). Only the SFDP read uses an address
             // on the measured paths and its answer does not depend on it; the lockstep settles it.
-            self.register(ADDR) & u32::MAX.checked_shr(u32::BITS - bits).unwrap_or(u32::MAX)
+            FlashAddress {
+                value: self.register(ADDR)
+                    & u32::MAX.checked_shr(u32::BITS - bits).unwrap_or(u32::MAX),
+                bits,
+            }
         });
         let dummy_cycles = if user & USR_DUMMY == 0 {
             0
@@ -320,10 +425,7 @@ impl Spi1 {
         let data_out: Vec<u8> = if user & USR_MOSI == 0 {
             Vec::new()
         } else {
-            (0..DATA_WORDS)
-                .flat_map(|word| self.register(W0 + 4 * word).to_le_bytes())
-                .take(self.data_len(MOSI_DLEN))
-                .collect()
+            self.data_out(self.data_len(MOSI_DLEN))
         };
         let data_in_len = if user & USR_MISO == 0 {
             0
@@ -352,28 +454,44 @@ impl Spi1 {
         }
     }
 
-    /// Runs every command whose bit is set. Transfers complete at once, so `CMD` reads 0 afterwards.
+    /// Runs the command whose bit is set in a write of `CMD`: one write, one transfer. Transfers
+    /// complete at once, so `CMD` reads 0 afterwards.
     ///
     /// `CMD` bit 17, `FLASH_PE`, is not a command: "In user mode it is set to indicate that
     /// program/erase operation will be triggered. The bit is combined with `spi_mem_usr` bit" (ESP-IDF
     /// v5.5.2 `components/soc/esp32c3/register/soc/spi_mem_reg.h:113-116`). It accompanies the bit
-    /// that starts the transfer: the rev3 ROM's `SPI_sector_erase` stores `0x01020000`, `FLASH_SE |
-    /// FLASH_PE` (`lui a4,0x1020` at `0x4004d296`, store at `0x4004d29a`), and ESP-IDF's
-    /// `spimem_flash_ll_user_start(dev, pe_ops)` ORs in `0x60000`, `USR | FLASH_PE`, for every
-    /// program or erase (`components/hal/esp32c3/include/hal/spimem_flash_ll.h:391-395`). This model
-    /// has no use for the indication (program/erase suspend is not modelled: `SUS_STATUS` is
-    /// storage), so the flag changes nothing: the command bit it comes with runs once, as without it.
+    /// that starts the transfer, and not always: the rev3 ROM's `SPI_sector_erase` stores
+    /// `0x01020000`, `FLASH_SE | FLASH_PE` (`lui a4,0x1020` at `0x4004d296`, store at `0x4004d29a`),
+    /// and its page program `0x02000000`, `FLASH_PP` alone; ESP-IDF erases with `FLASH_SE` alone
+    /// (`0x01000000`) and programs with `FLASH_PP` alone and `ADDR = len << 24 | addr` (measured:
+    /// reports FW key result 1, SF key result 12). ESP-IDF's `spimem_flash_ll_user_start(dev,
+    /// pe_ops)` ORs in `0x60000`, `USR | FLASH_PE`, only for the program and erase commands that
+    /// take a 4-byte address, which it sends above 16 MiB
+    /// (`components/hal/esp32c3/include/hal/spimem_flash_ll.h:391-395`,
+    /// `components/spi_flash/spi_flash_chip_generic.c:118-151`; FW §B.1): no measured boot contains
+    /// that transfer. This model has no use for the indication (a flash operation is never
+    /// suspended), so the flag changes nothing: the command bit it comes with runs once, as
+    /// without it.
+    ///
+    /// Several command bits in one write are refused by name and nothing is sent: no ROM or ESP-IDF
+    /// code writes more than one, the order in which the controller would run them is not
+    /// documented, and a guess would now write the flash.
     ///
     /// ASSUMPTION: `FLASH_PE` written without any command bit starts no transfer and self-clears. No
     /// ROM or ESP-IDF v5.5.2 code writes it alone; the SP1b-4 lockstep, or a device, settles it.
-    fn run(&mut self, ctx: &mut Ctx, command_bits: u32) {
-        for (bit, opcode) in HARD_WIRED {
-            if command_bits & bit != 0 {
-                self.run_hard_wired(ctx, bit, opcode);
+    fn run(&mut self, ctx: &mut Ctx, value: u32) {
+        let commands = value & COMMAND_BITS;
+        match commands.count_ones() {
+            0 => {}
+            1 if commands == CMD_USR => self.run_user(ctx),
+            1 => {
+                // INVARIANT: `HARD_WIRED` has an entry for each of the bits 31 to 19, and
+                // `commands` is one of them.
+                if let Some(&(bit, opcode)) = HARD_WIRED.iter().find(|(bit, _)| *bit == commands) {
+                    self.run_hard_wired(ctx, bit, opcode);
+                }
             }
-        }
-        if command_bits & CMD_USR != 0 {
-            self.run_user(ctx);
+            _ => ctx.unsupported(Unsupported::FlashCommandBits { cmd: value }),
         }
     }
 }
@@ -392,6 +510,18 @@ impl Peripheral<Ctx> for Spi1 {
             self.run(ctx, value);
             return true;
         }
+        if offset == FLASH_SUS_CTRL {
+            // Suspending is not modelled: an operation completes inside the write that starts
+            // it (report FW §A.5), so there is never one to suspend. ESP-IDF writes this register
+            // at every bus acquisition with every one of these bits clear and only a time-out
+            // count (`0x0A000000`: FW §A.2), and refuses to enable auto-suspend on the modelled
+            // chip (FW §B.2); a write that asks for a suspend or a resume is reported.
+            if value & SUS_REQUESTS != 0 {
+                ctx.unsupported(Unsupported::FlashSuspend { sus_ctrl: value });
+            }
+            // The two command bits clear themselves.
+            return self.registers.write(offset, value & !SUS_COMMANDS);
+        }
         self.registers.write(offset, value)
     }
 }
````

- [ ] **Step 6: Run the suite, regenerate the table**

```bash
INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t2-bless.log 2>&1; echo "exit=$?"
cargo test --workspace > /tmp/t2.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t2.log
```

Expected: `exit=0` twice and `847`. The table, read before it is committed:

````diff
--- a/docs/registers-accepted-without-effect.md
+++ b/docs/registers-accepted-without-effect.md
@@ -31,8 +31,8 @@ then rewrites the table below, and nothing else; review the diff before committi
 
 "Count" is 1 for a single register, or the number of consecutive registers the row stands for.
 "Made real by" names the plan that gives the register its effect. Each plan inherits the rows that
-name it. The table has eight heirs: SP1b-3b (flash writes), SP1b-3d (the serial consoles in both
-directions), SP1b-3f (deep sleep), SP1b-4, SP2, SP5a, SP5b and SP8. The carry-over note
+name it. The table has seven heirs: SP1b-3d (the serial consoles in both directions), SP1b-3f
+(deep sleep), SP1b-4, SP2, SP5a, SP5b and SP8. The carry-over note
 `docs/superpowers/notes/2026-10-11-sp1b-3a-carryover.md` names the rows one by one, as the table
 stood at the end of plan SP1b-3a, when one plan, "SP1b-3b", was to take what five plans now share;
 its first section says which plan takes what.
@@ -44,14 +44,14 @@ its first section says which plan takes what.
 | `0x60000010` | `UART0.INT_CLR` | 1 | interrupt clear: nothing is pending | SP1b-3d |
 | `0x60000020` | `UART0.CONF0` | 1 | frame format: a byte is 10 bit times (the transmit FIFO reset acts) | SP1b-3d |
 | `0x60000024` | `UART0.CONF1` | 1 | FIFO thresholds | SP1b-3d |
-| `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level | SP8 |
+| `0x60002008` | `SPI1.CTRL` | 1 | read mode, dummy output, WP level (the two-byte status write bit acts) | SP8 |
 | `0x6000200c` | `SPI1.CTRL1` | 1 | CS hold delay after a release-from-power-down | SP8 |
 | `0x60002010` | `SPI1.CTRL2` | 1 | CS setup and hold times | SP8 |
 | `0x60002014` | `SPI1.CLOCK` | 1 | SPI clock divider: a transfer takes no time | SP8 |
 | `0x60002034` | `SPI1.MISC` | 1 | chip-select idle level and enables | SP8 |
-| `0x60002098` | `SPI1.FLASH_WAITI_CTRL` | 1 | automatic wait for the chip to be idle after a program or an erase | SP1b-3b |
-| `0x6000209c` | `SPI1.FLASH_SUS_CTRL` | 1 | automatic program/erase suspend and resume | SP1b-3b |
-| `0x600020a4` | `SPI1.SUS_STATUS` | 1 | program/erase suspend state | SP1b-3b |
+| `0x60002098` | `SPI1.FLASH_WAITI_CTRL` | 1 | automatic wait for the chip to be idle after a program or an erase: an operation completes at once | SP8 |
+| `0x6000209c` | `SPI1.FLASH_SUS_CTRL` | 1 | program/erase suspend and resume: an operation completes at once (a suspend or resume request is refused by name) | SP8 |
+| `0x600020a4` | `SPI1.SUS_STATUS` | 1 | program/erase suspend state: nothing is ever suspended | SP8 |
 | `0x600020dc` | `SPI1.CLOCK_GATE` | 1 | register clock gate | SP8 |
 | `0x60003008` | `SPI0.CTRL` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
 | `0x60003010` | `SPI0.CTRL2` | 1 | cache-fill transfer (read command, dummy cycles, clock): the bus reads the flash directly | SP1b-4 |
````

Ignored tests (the boot does not change: the ROM and the bootloader send one command per write):

```bash
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t2-i1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 10 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware > /tmp/t2-i2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 7 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t2-i3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 2 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored > /tmp/t2-i4.log 2>&1; echo "exit=$?"
target/release/inkemu run --device x4 --strict-mmio --max-cycles 40000000 --trace-hash --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" > /dev/null 2> /tmp/t2-hash.log; echo "exit=$?"; grep trace-hash /tmp/t2-hash.log
```

Expected: `exit=0` five times and `trace-hash 10174cc2726c3d77`.

- [ ] **Step 7: Mutations**

Each row: apply the change to `periph/spi_mem.rs`, run `cargo test -p soc-esp32c3 --lib`, see the test fail, undo.

| Mutation | Test that fails |
|---|---|
| `value: addr & HARD_WIRED_ADDRESS_MASK,` → `value: addr,` | `a_hard_wired_program_sends_its_address_and_the_bytes_its_count_names`, `sector_and_block_erase_send_the_low_24_bits_of_addr_with_or_without_the_flag` |
| `if len == 0 \|\| len > DATA_BYTES {` → `if len == 0 {`, then → `if len > DATA_BYTES {` | `a_hard_wired_program_of_no_byte_or_of_more_than_64_is_refused_and_sends_nothing` (both) |
| the arm `_ => ctx.unsupported(Unsupported::FlashCommandBits { cmd: value })` replaced by a loop that runs every set bit of `HARD_WIRED` | `one_write_of_cmd_with_several_command_bits_runs_none_and_says_so`, `bus::tests::a_write_of_several_flash_command_bits_is_one_named_diagnostic_and_a_strict_violation` |
| `if value & SUS_REQUESTS != 0 {` → `if value & SUS_COMMANDS != 0 {` | `a_suspend_or_resume_request_is_refused_by_name_and_the_measured_configuration_is_stored` |
| `value & !SUS_COMMANDS` → `value` in the write of `FLASH_SUS_CTRL` | the same test |
| `& CTRL_WRSR_2B == 0 {` → `& CTRL_WRSR_2B == CTRL_WRSR_2B {` | `the_status_write_sends_one_byte_of_rd_status_or_two_when_ctrl_says_so` |
| in the `CMD_SE \| CMD_BE` arm, `&FlashCommand { address, ..command }` → `&command` | `sector_and_block_erase_send_the_low_24_bits_of_addr_with_or_without_the_flag`, `the_rom_sector_erase_sets_the_program_erase_flag_and_is_one_command` |
| `const COMMAND_BITS: u32 = !0 << 18;` → `!0 << 17` | `the_rom_sector_erase_sets_the_program_erase_flag_and_is_one_command`, `a_user_transfer_that_carries_the_program_erase_flag_runs_once` |
| in the `CMD_READ` arm, `&FlashCommand { address, ..command }` → `&command` | `the_hard_wired_read_sends_its_address_and_stores_what_the_chip_answers` |

- [ ] **Step 8: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(soc): SPI1 hard-wired commands carry their address and data; one command per write of CMD

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src docs/registers-accepted-without-effect.md
```

---
## Task 3: The flash chip takes programs, erases and status writes; the OTA layout writes `otadata`

The chip model refused every command that changes its content. It now programs, erases and stores its status register, under the write-enable latch, and the controller keeps a journal of what was written. Two ignored tests of the X4 partition layout were built on the refusal of the `otadata` update: they are rewritten to hold the opposite, with the measured bytes.

**Files:**
- Modify: `crates/soc-esp32c3/src/flash.rs` (the write commands, `FlashRefusal`, `FlashWrite`, `FlashJournal`, generation, modified sectors)
- Modify: `crates/soc-esp32c3/src/periph/spi_mem.rs` (refusals by kind, the journal)
- Modify: `crates/soc-esp32c3/src/diag.rs` (three refusals of the chip)
- Modify: `crates/soc-esp32c3/src/machine.rs` (`take_flash_writes`, `flash_writes_dropped`)
- Modify: `crates/soc-esp32c3/src/lib.rs` (re-export of `FlashWrite`)
- Modify: `crates/soc-esp32c3/src/test_support.rs` (`flash_program`, `flash_erase_sector`)
- Modify: `crates/soc-esp32c3/src/bus.rs` (tests)
- Modify: `crates/soc-esp32c3/tests/reference_boot.rs` (the two OTA tests)

**Interfaces:**
- Consumes: `FlashAddress`, `FlashCommand` (Task 2); `Unsupported::FlashCommand { opcode }`; `Ctx::unsupported`.
- Produces:
  - `pub const SECTOR_BYTES: u32 = 4 << 10`, `BLOCK_BYTES: u32 = 64 << 10`, `PAGE_BYTES: u32 = 256`, `JOURNAL_CAPACITY: usize = 1024` in `soc_esp32c3::flash`.
  - `pub enum FlashRefusal { Unmodeled { opcode: u8 }, Form { opcode: u8, address_bits: u32, dummy_cycles: u32, data_bytes: u32 }, OutOfRange { opcode: u8, address: u32, len: u32 }, Protected { opcode: u8, status: u8 } }` (replaces `UnmodeledCommand`).
  - `pub enum FlashWrite { Program { address: u32, data: Vec<u8> }, Erase { start: u32, len: u32 }, Status { value: u8 } }`, re-exported as `soc_esp32c3::FlashWrite`.
  - `pub struct FlashJournal` with `pub(crate) fn record(&mut self, write: FlashWrite)`, `pub fn take(&mut self) -> Vec<FlashWrite>`, `pub const fn dropped(&self) -> u64`.
  - `trait FlashDevice`: `fn execute(&mut self, command: &FlashCommand<'_>, data_in: &mut [u8]) -> Result<Option<FlashWrite>, FlashRefusal>`, and two new methods, `fn generation(&self) -> u64` (moves when a byte of the content changes, and only then) and `fn modified_sectors(&self) -> u32` (4 KiB sectors a write changed since the chip was built).
  - `Unsupported::FlashCommandForm { opcode, address_bits, dummy_cycles, data_bytes }`, `FlashRange { opcode, address, len }`, `FlashProtected { opcode, status }` (keys 18, 19, 20) and `impl From<FlashRefusal> for Unsupported`.
  - `Spi1::take_flash_writes(&mut self) -> Vec<FlashWrite>`, `Spi1::flash_writes_dropped(&self) -> u64`; the same two on `Esp32C3`.
  - `test_support::flash_program(address: u32, data: &[u8]) -> Vec<u32>` and `flash_erase_sector(address: u32) -> Vec<u32>`: machine code that enables writing and runs the hard-wired command.

**Bounds on what the firmware writes.** The chip takes at most the last 256 bytes of a program (the controller sends at most 64): `skip = len.saturating_sub(256)`. An address is refused when it is at or past `config.size` before anything is indexed, so the image grows to at most the size of the chip (`MAX_FLASH_BYTES`). An erase covers a constant unit (4 KiB, 64 KiB) or the chip. The journal keeps 1,024 entries of at most 256 bytes each and counts the rest. The modified-sector set is a fixed array of bits, one per sector of the largest chip.

- [ ] **Step 1: Write the failing tests**

The helpers of `test_support.rs` first (they are code, used by the tests of this task and of later ones):

````diff
--- a/crates/soc-esp32c3/src/test_support.rs
+++ b/crates/soc-esp32c3/src/test_support.rs
@@ -369,6 +369,46 @@ pub fn stop_watchdogs() -> Vec<u32> {
     program
 }
 
+/// `SPI1.CMD` bits of the hard-wired commands the two helpers below use (report FW §A.1).
+const SPI1_CMD_WREN: u32 = 1 << 30;
+const SPI1_CMD_PP: u32 = 1 << 25;
+const SPI1_CMD_SE: u32 = 1 << 24;
+
+/// Code that programs `data` (1 to 64 bytes) into the flash at `address`, the way the ROM and
+/// ESP-IDF do: write enable, the bytes in `W0..`, `ADDR = len << 24 | address`, `CMD = FLASH_PP`
+/// (reports FW §A.2, §A.3).
+#[must_use]
+pub fn flash_program(address: u32, data: &[u8]) -> Vec<u32> {
+    assert!((1..=64).contains(&data.len()), "1 to 64 bytes");
+    assert!(address < 1 << 24, "a 24-bit address");
+    let spi1 = |register| svd_regs::esp32c3::addr("SPI1", register);
+    let mut program = store_register(spi1("CMD"), SPI1_CMD_WREN).to_vec();
+    for (word, chunk) in (0u32..).zip(data.chunks(4)) {
+        let mut le = [0u8; 4];
+        le[..chunk.len()].copy_from_slice(chunk);
+        program.extend(store_register(
+            spi1("W0") + 4 * word,
+            u32::from_le_bytes(le),
+        ));
+    }
+    let len = u32::try_from(data.len()).unwrap();
+    program.extend(store_register(spi1("ADDR"), (len << 24) | address));
+    program.extend(store_register(spi1("CMD"), SPI1_CMD_PP));
+    program
+}
+
+/// Code that erases the 4 KiB flash sector holding `address`: write enable, `ADDR`,
+/// `CMD = FLASH_SE`, as ESP-IDF does (report FW §A.2).
+#[must_use]
+pub fn flash_erase_sector(address: u32) -> Vec<u32> {
+    assert!(address < 1 << 24, "a 24-bit address");
+    let spi1 = |register| svd_regs::esp32c3::addr("SPI1", register);
+    let mut program = store_register(spi1("CMD"), SPI1_CMD_WREN).to_vec();
+    program.extend(store_register(spi1("ADDR"), address));
+    program.extend(store_register(spi1("CMD"), SPI1_CMD_SE));
+    program
+}
+
 /// Runs `work` on its own thread and waits at most `limit` for its result: a test of a loop that
 /// must end, which would otherwise hang the whole suite if the loop did not. On a timeout the
 /// test fails with `unbounded`; the thread is left to spin until the test process ends. A panic of
````

The chip, with the bytes measured on Espressif's code: the `otadata` entry of the bootloader (FW §E) and the NVS rows of a first boot (SF §B.2):

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -313,7 +625,10 @@ mod tests {
 
     const MIB: usize = 1 << 20;
 
-    fn run(flash: &mut NorFlash, opcode: u8, data_in: &mut [u8]) -> Result<(), UnmodeledCommand> {
+    /// What [`FlashDevice::execute`] returns.
+    type Done = Result<Option<FlashWrite>, FlashRefusal>;
+
+    fn run(flash: &mut NorFlash, opcode: u8, data_in: &mut [u8]) -> Done {
         flash.execute(
             &FlashCommand {
                 opcode,
@@ -325,6 +640,63 @@ mod tests {
         )
     }
 
+    /// A command with a 24-bit address and data, no dummy clock: the form of a program and of
+    /// an erase.
+    fn send(flash: &mut NorFlash, opcode: u8, address: u32, data: &[u8]) -> Done {
+        flash.execute(
+            &FlashCommand {
+                opcode,
+                address: Some(FlashAddress {
+                    value: address,
+                    bits: 24,
+                }),
+                dummy_cycles: 0,
+                data_out: data,
+            },
+            &mut [],
+        )
+    }
+
+    /// Write enable, then the command: how the ROM and ESP-IDF send every program and erase.
+    fn write(flash: &mut NorFlash, opcode: u8, address: u32, data: &[u8]) -> Done {
+        run(flash, 0x06, &mut []).unwrap();
+        send(flash, opcode, address, data)
+    }
+
+    /// A command without an address that carries `data`: the status write.
+    fn send_data(flash: &mut NorFlash, opcode: u8, data: &[u8]) -> Done {
+        flash.execute(
+            &FlashCommand {
+                opcode,
+                address: None,
+                dummy_cycles: 0,
+                data_out: data,
+            },
+            &mut [],
+        )
+    }
+
+    fn content(flash: &NorFlash, offset: u32, len: usize) -> Vec<u8> {
+        let mut bytes = vec![0u8; len];
+        flash.read(offset, &mut bytes);
+        bytes
+    }
+
+    fn status(flash: &mut NorFlash) -> u8 {
+        let mut status = [0u8];
+        run(flash, 0x05, &mut status).unwrap();
+        status[0]
+    }
+
+    /// The bytes written as hexadecimal pairs; spaces are ignored.
+    fn hex(text: &str) -> Vec<u8> {
+        let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
+        digits
+            .chunks(2)
+            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
+            .collect()
+    }
+
     #[test]
     fn a_blank_flash_reads_erased_everywhere_even_past_its_end() {
         let flash = NorFlash::blank(FlashConfig::default());
@@ -510,19 +882,30 @@ mod tests {
     }
 
     #[test]
-    fn commands_that_would_change_or_read_the_content_are_not_modeled() {
+    fn data_reads_and_opcodes_the_model_does_not_know_are_refused_by_opcode() {
+        // The data reads, and what no measured boot sends: a second status register (`31`,
+        // `35`), a 32 KiB erase (`52`: no caller in ESP-IDF, SF §C), reset (`66`, `99`),
+        // suspend (`75`), deep power-down (`B9`).
         let mut flash = NorFlash::with_image(vec![1, 2, 3, 4], FlashConfig::default()).unwrap();
+        run(&mut flash, 0x06, &mut []).unwrap();
         for opcode in [
-            0x02, 0x20, 0xD8, 0xC7, 0x01, 0x31, 0x35, 0x03, 0xBB, 0x00, 0xFF,
+            0x03, 0x0B, 0xBB, 0x31, 0x35, 0x52, 0x66, 0x99, 0x75, 0xB9, 0x00, 0xFF,
         ] {
             assert_eq!(
                 run(&mut flash, opcode, &mut [0u8; 4]),
-                Err(UnmodeledCommand { opcode })
+                Err(FlashRefusal::Unmodeled { opcode })
+            );
+            assert_eq!(
+                send(&mut flash, opcode, 0, &[0]),
+                Err(FlashRefusal::Unmodeled { opcode })
             );
         }
-        let mut bytes = [0u8; 4];
-        flash.read(0, &mut bytes);
-        assert_eq!(bytes, [1, 2, 3, 4], "the content did not change");
+        assert_eq!(
+            content(&flash, 0, 4),
+            [1, 2, 3, 4],
+            "the content did not change"
+        );
+        assert_eq!(status(&mut flash), 0x02, "nor did the latch");
     }
 
     #[test]
@@ -531,11 +914,413 @@ mod tests {
         let mut data_in = [0x5A, 0xA5, 0x3C];
         assert_eq!(
             run(&mut flash, 0x03, &mut data_in),
-            Err(UnmodeledCommand { opcode: 0x03 })
+            Err(FlashRefusal::Unmodeled { opcode: 0x03 })
         );
         assert_eq!(data_in, [0x5A, 0xA5, 0x3C], "nothing was clocked in");
     }
 
+    #[test]
+    fn a_program_needs_the_write_enable_latch_and_clears_it_when_it_completes() {
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        // Without the latch a program is ignored, as the datasheet says (FW §C).
+        assert_eq!(send(&mut flash, 0x02, 0x9000, &[0x12]), Ok(None));
+        assert_eq!(content(&flash, 0x9000, 1), [0xFF]);
+        assert_eq!(
+            write(&mut flash, 0x02, 0x9000, &[0x12]),
+            Ok(Some(FlashWrite::Program {
+                address: 0x9000,
+                data: vec![0x12]
+            }))
+        );
+        assert_eq!(content(&flash, 0x9000, 1), [0x12]);
+        assert_eq!(
+            status(&mut flash),
+            0,
+            "WEL reads 0 after a program (FW key result 5)"
+        );
+        // The latch served once: a second program needs a second write enable.
+        assert_eq!(send(&mut flash, 0x02, 0x9001, &[0x34]), Ok(None));
+        assert_eq!(content(&flash, 0x9001, 1), [0xFF]);
+    }
+
+    #[test]
+    fn a_program_only_clears_bits() {
+        // NVS rewrites its bitmap word in place: `FE`, then `FA` (SF §B.2).
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        write(&mut flash, 0x02, 0x9020, &[0xFE, 0xFF, 0xFF, 0xFF]).unwrap();
+        write(&mut flash, 0x02, 0x9020, &[0xFA, 0xFF, 0xFF, 0xFF]).unwrap();
+        assert_eq!(content(&flash, 0x9020, 4), [0xFA, 0xFF, 0xFF, 0xFF]);
+        // No program sets a bit again: `FA AND FF`, then `FA AND 0F`.
+        write(&mut flash, 0x02, 0x9020, &[0xFF]).unwrap();
+        assert_eq!(content(&flash, 0x9020, 1), [0xFA]);
+        write(&mut flash, 0x02, 0x9020, &[0x0F]).unwrap();
+        assert_eq!(content(&flash, 0x9020, 1), [0x0A]);
+    }
+
+    #[test]
+    fn a_program_wraps_inside_its_256_byte_page() {
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        write(&mut flash, 0x02, 0x10FE, &[1, 2, 3, 4]).unwrap();
+        assert_eq!(content(&flash, 0x10FE, 2), [1, 2]);
+        assert_eq!(
+            content(&flash, 0x1000, 2),
+            [3, 4],
+            "the page counter wraps to the start of the page"
+        );
+        assert_eq!(
+            content(&flash, 0x1100, 2),
+            [0xFF, 0xFF],
+            "the next page is not reached"
+        );
+    }
+
+    #[test]
+    fn of_more_than_a_page_of_data_the_last_256_bytes_are_programmed() {
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        // 300 bytes that cycle through 0 to 254, so none of them is `FF`: 44 are dropped.
+        let data: Vec<u8> = (0u8..=254).cycle().take(300).collect();
+        assert_eq!(
+            write(&mut flash, 0x02, 0x2010, &data),
+            Ok(Some(FlashWrite::Program {
+                address: 0x203C,
+                data: data[44..].to_vec()
+            })),
+            "the first byte kept, byte 44, lands at 0x10 + 44"
+        );
+        assert_eq!(content(&flash, 0x203C, 1), [data[44]]);
+        assert_eq!(
+            content(&flash, 0x2000, 1),
+            [data[240]],
+            "0x10 + 240 wraps to 0"
+        );
+        assert_eq!(
+            content(&flash, 0x203B, 1),
+            [data[299]],
+            "the last byte ends the page turn"
+        );
+        assert!(
+            content(&flash, 0x2000, 256)
+                .iter()
+                .all(|&byte| byte != 0xFF),
+            "every position of the page was programmed once"
+        );
+        assert_eq!(content(&flash, 0x2100, 1), [0xFF]);
+    }
+
+    /// The `otadata` entry the stock bootloader writes on the first boot of an OTA layout
+    /// (FW §E, first row): sequence 1, a blank label, state valid, CRC.
+    fn otadata_entry() -> Vec<u8> {
+        let mut entry = vec![0xFF; 32];
+        entry[..4].copy_from_slice(&[1, 0, 0, 0]);
+        entry[24..28].copy_from_slice(&[2, 0, 0, 0]);
+        entry[28..].copy_from_slice(&[0x9A, 0x98, 0x43, 0x47]);
+        entry
+    }
+
+    #[test]
+    fn the_otadata_update_of_the_bootloader_leaves_the_measured_bytes() {
+        // FW §E: the ROM's sector erase at 0xE000, then its 32-byte program at 0xE000.
+        let mut flash = NorFlash::with_image(vec![0x55; 0x1_0000], FlashConfig::default()).unwrap();
+        assert_eq!(
+            write(&mut flash, 0x20, 0xE000, &[]),
+            Ok(Some(FlashWrite::Erase {
+                start: 0xE000,
+                len: 0x1000
+            }))
+        );
+        assert_eq!(
+            write(&mut flash, 0x02, 0xE000, &otadata_entry()),
+            Ok(Some(FlashWrite::Program {
+                address: 0xE000,
+                data: otadata_entry()
+            }))
+        );
+        assert_eq!(content(&flash, 0xE000, 32), otadata_entry());
+        assert_eq!(
+            content(&flash, 0xE020, 0xFE0),
+            vec![0xFF; 0xFE0],
+            "the rest of the sector is erased"
+        );
+        assert_eq!(content(&flash, 0xDFFC, 4), [0x55; 4], "the sector before");
+        assert_eq!(content(&flash, 0xF000, 4), [0x55; 4], "the sector after");
+        assert_eq!(flash.modified_sectors(), 1);
+    }
+
+    #[test]
+    fn the_nvs_programs_of_a_first_boot_leave_the_measured_rows() {
+        // SF §B.2, boot 1: five programs on an erased partition, no erase. The bitmap word at
+        // 0x9020 is programmed twice.
+        let header = format!("feffffff 00000000 fe {} 842dbab9", "ff".repeat(19));
+        let namespace = "000101ff 09a95007 73746f72 61676500 00000000 00000000 01ffffff ffffffff";
+        let counter = "011401ff 81a8ff42 72657374 6172745f 636f756e 74657200 01000000 ffffffff";
+        let programs = [
+            (0x9000, hex(&header)),
+            (0x9040, hex(namespace)),
+            (0x9020, hex("feffffff")),
+            (0x9060, hex(counter)),
+            (0x9020, hex("faffffff")),
+        ];
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        for (address, data) in &programs {
+            assert_eq!(
+                write(&mut flash, 0x02, *address, data),
+                Ok(Some(FlashWrite::Program {
+                    address: *address,
+                    data: data.clone()
+                }))
+            );
+        }
+        let bitmap = format!("faffffff {}", "ff".repeat(28));
+        let rows = [hex(&header), hex(&bitmap), hex(namespace), hex(counter)].concat();
+        assert_eq!(
+            content(&flash, 0x9000, 0x80),
+            rows,
+            "the four rows of SF §B.2"
+        );
+        assert_eq!(content(&flash, 0x9080, 0x80), vec![0xFF; 0x80]);
+        assert_eq!(flash.modified_sectors(), 1, "all in flash sector 9");
+    }
+
+    #[test]
+    fn a_sector_erase_and_a_block_erase_set_their_whole_unit_and_nothing_else() {
+        let mut flash = NorFlash::with_image(vec![0u8; 0x4_0000], FlashConfig::default()).unwrap();
+        // The low address bits are ignored (FW §G): 0x12345 is in the sector of 0x12000.
+        assert_eq!(
+            write(&mut flash, 0x20, 0x1_2345, &[]),
+            Ok(Some(FlashWrite::Erase {
+                start: 0x1_2000,
+                len: 0x1000
+            }))
+        );
+        assert_eq!(content(&flash, 0x1_1FFF, 1), [0]);
+        assert_eq!(content(&flash, 0x1_2000, 0x1000), vec![0xFF; 0x1000]);
+        assert_eq!(content(&flash, 0x1_3000, 1), [0]);
+        assert_eq!(flash.modified_sectors(), 1);
+        assert_eq!(
+            write(&mut flash, 0xD8, 0x2_FFFF, &[]),
+            Ok(Some(FlashWrite::Erase {
+                start: 0x2_0000,
+                len: 0x1_0000
+            }))
+        );
+        assert_eq!(content(&flash, 0x1_FFFF, 1), [0]);
+        assert_eq!(content(&flash, 0x2_0000, 0x1_0000), vec![0xFF; 0x1_0000]);
+        assert_eq!(content(&flash, 0x3_0000, 1), [0]);
+        assert_eq!(
+            flash.modified_sectors(),
+            17,
+            "one sector, then the sixteen of a block"
+        );
+        assert_eq!(status(&mut flash), 0, "the latch is clear after an erase");
+    }
+
+    #[test]
+    fn a_chip_erase_erases_everything_under_either_opcode() {
+        for opcode in [0xC7, 0x60] {
+            let mut flash =
+                NorFlash::with_image(vec![0u8; 0x2800], FlashConfig::default()).unwrap();
+            run(&mut flash, 0x06, &mut []).unwrap();
+            assert_eq!(
+                run(&mut flash, opcode, &mut []),
+                Ok(Some(FlashWrite::Erase {
+                    start: 0,
+                    len: 16 << 20
+                })),
+                "{opcode:#04x}"
+            );
+            assert_eq!(content(&flash, 0, 0x2800), vec![0xFF; 0x2800]);
+            assert_eq!(
+                flash.modified_sectors(),
+                3,
+                "the three sectors that held something"
+            );
+            assert_eq!(status(&mut flash), 0);
+        }
+    }
+
+    #[test]
+    fn an_erase_without_the_latch_changes_nothing() {
+        let mut flash = NorFlash::with_image(vec![0u8; 0x2_0000], FlashConfig::default()).unwrap();
+        assert_eq!(send(&mut flash, 0x20, 0, &[]), Ok(None));
+        assert_eq!(send(&mut flash, 0xD8, 0x1_0000, &[]), Ok(None));
+        assert_eq!(run(&mut flash, 0xC7, &mut []), Ok(None));
+        assert_eq!(run(&mut flash, 0x60, &mut []), Ok(None));
+        assert_eq!(content(&flash, 0, 0x2_0000), vec![0u8; 0x2_0000]);
+        assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
+    }
+
+    #[test]
+    fn an_operation_that_changes_no_byte_is_performed_and_modifies_nothing() {
+        // An erase of an erased sector and a program of `FF` are operations of the journal; the
+        // content, its generation and the modified sectors do not move.
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        assert!(matches!(
+            write(&mut flash, 0x20, 0x9000, &[]),
+            Ok(Some(FlashWrite::Erase { .. }))
+        ));
+        assert!(matches!(
+            write(&mut flash, 0x02, 0x9000, &[0xFF, 0xFF]),
+            Ok(Some(FlashWrite::Program { .. }))
+        ));
+        assert_eq!((flash.generation(), flash.modified_sectors()), (0, 0));
+        write(&mut flash, 0x02, 0x9000, &[0xFE]).unwrap();
+        let after_a_change = flash.generation();
+        assert_ne!(after_a_change, 0);
+        assert_eq!(flash.modified_sectors(), 1);
+        // The same bytes again: nothing changes.
+        write(&mut flash, 0x02, 0x9000, &[0xFE]).unwrap();
+        assert_eq!(flash.generation(), after_a_change);
+        write(&mut flash, 0x20, 0x9000, &[]).unwrap();
+        assert_ne!(
+            flash.generation(),
+            after_a_change,
+            "the erase changed a byte"
+        );
+        assert_eq!(flash.modified_sectors(), 1, "the same sector");
+        // The image now reaches 0x9100, and what it holds below sector 9 is erased: erasing
+        // that again, or sector 9 a second time, changes no byte.
+        let erased = flash.generation();
+        write(&mut flash, 0x20, 0x8000, &[]).unwrap();
+        write(&mut flash, 0x20, 0x9000, &[]).unwrap();
+        write(&mut flash, 0xD8, 0x0000, &[]).unwrap();
+        assert_eq!((flash.generation(), flash.modified_sectors()), (erased, 1));
+    }
+
+    #[test]
+    fn the_status_register_stores_its_writable_bits_and_its_write_needs_and_clears_the_latch() {
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        assert_eq!(send_data(&mut flash, 0x01, &[0x40]), Ok(None), "no latch");
+        assert_eq!(status(&mut flash), 0);
+        run(&mut flash, 0x06, &mut []).unwrap();
+        assert_eq!(
+            send_data(&mut flash, 0x01, &[0xFF]),
+            Ok(Some(FlashWrite::Status { value: 0xFC })),
+            "WIP and WEL are not stored"
+        );
+        assert_eq!(
+            status(&mut flash),
+            0xFC,
+            "the latch is clear, the rest reads back"
+        );
+        run(&mut flash, 0x06, &mut []).unwrap();
+        assert_eq!(status(&mut flash), 0xFE);
+        send_data(&mut flash, 0x01, &[0x00]).unwrap();
+        assert_eq!(status(&mut flash), 0);
+    }
+
+    #[test]
+    fn a_program_or_an_erase_under_a_block_protect_bit_is_refused_by_name() {
+        for bp in [0x04u8, 0x08, 0x10, 0x20, 0x3C] {
+            let mut flash = NorFlash::with_image(vec![0x0F; 16], FlashConfig::default()).unwrap();
+            run(&mut flash, 0x06, &mut []).unwrap();
+            send_data(&mut flash, 0x01, &[bp]).unwrap();
+            let protected = |opcode| {
+                Err(FlashRefusal::Protected {
+                    opcode,
+                    status: bp | 0x02,
+                })
+            };
+            assert_eq!(write(&mut flash, 0x02, 0, &[0x00]), protected(0x02));
+            assert_eq!(write(&mut flash, 0x20, 0, &[]), protected(0x20));
+            assert_eq!(write(&mut flash, 0xD8, 0, &[]), protected(0xD8));
+            assert_eq!(run(&mut flash, 0xC7, &mut []), protected(0xC7));
+            assert_eq!(content(&flash, 0, 16), [0x0F; 16], "status {bp:#04x}");
+            assert_eq!(flash.modified_sectors(), 0);
+            // The status write is how the protection is lifted: it is not protected itself.
+            send_data(&mut flash, 0x01, &[0x00]).unwrap();
+            assert!(write(&mut flash, 0x02, 0, &[0x00]).unwrap().is_some());
+            assert_eq!(content(&flash, 0, 1), [0x00]);
+        }
+        // Quad enable and SRWD protect nothing.
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        run(&mut flash, 0x06, &mut []).unwrap();
+        send_data(&mut flash, 0x01, &[0xC0]).unwrap();
+        assert!(write(&mut flash, 0x02, 0, &[0x00]).unwrap().is_some());
+    }
+
+    #[test]
+    fn a_write_past_the_end_of_a_smaller_chip_is_refused_by_name() {
+        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
+        let mut flash = NorFlash::blank(small);
+        for (opcode, data, len) in [
+            (0x02u8, &[0u8, 1][..], 2u32),
+            (0x20, &[][..], 0x1000),
+            (0xD8, &[][..], 0x1_0000),
+        ] {
+            for address in [0x10_0000, 0xFF_FFFF] {
+                assert_eq!(
+                    write(&mut flash, opcode, address, data),
+                    Err(FlashRefusal::OutOfRange {
+                        opcode,
+                        address,
+                        len
+                    })
+                );
+            }
+        }
+        assert_eq!(flash.modified_sectors(), 0);
+        run(&mut flash, 0x04, &mut []).unwrap();
+        assert_eq!(
+            send(&mut flash, 0x02, 0x10_0000, &[0]),
+            Ok(None),
+            "without the latch a chip ignores the command wherever it points"
+        );
+        // The last byte of the chip is inside it: the image grows to the size of the chip and
+        // no further.
+        write(&mut flash, 0x02, 0x0F_FFFF, &[0x00, 0x11]).unwrap();
+        assert_eq!(content(&flash, 0x0F_FFFF, 1), [0x00]);
+        assert_eq!(
+            content(&flash, 0x0F_FF00, 1),
+            [0x11],
+            "wrapped inside the last page"
+        );
+        assert!(
+            format!("{flash:?}").contains("image_bytes: 1048576"),
+            "{flash:?}"
+        );
+    }
+
+    #[test]
+    fn a_write_command_in_a_form_the_chip_does_not_know_is_refused_by_name() {
+        let mut flash = NorFlash::with_image(vec![0x0F; 16], FlashConfig::default()).unwrap();
+        run(&mut flash, 0x06, &mut []).unwrap();
+        let at = |bits| Some(FlashAddress { value: 0, bits });
+        // (opcode, address, dummy clocks, data): each differs from the form of its command.
+        let frames: [(u8, Option<FlashAddress>, u32, &[u8]); 11] = [
+            (0x02, None, 0, &[0]),    // a program without an address
+            (0x02, at(32), 0, &[0]),  // with a 4-byte address
+            (0x02, at(16), 0, &[0]),  // with a 2-byte address
+            (0x02, at(24), 8, &[0]),  // with dummy clocks
+            (0x02, at(24), 0, &[]),   // without a byte
+            (0x20, None, 0, &[]),     // an erase without an address
+            (0x20, at(24), 0, &[0]),  // with data
+            (0xD8, at(32), 0, &[]),   // a block erase with a 4-byte address
+            (0xC7, at(24), 0, &[]),   // a chip erase with an address
+            (0x01, None, 0, &[0, 0]), // a status write of two bytes
+            (0x01, at(24), 0, &[0]),  // a status write with an address
+        ];
+        for (opcode, address, dummy_cycles, data_out) in frames {
+            let command = FlashCommand {
+                opcode,
+                address,
+                dummy_cycles,
+                data_out,
+            };
+            assert_eq!(
+                flash.execute(&command, &mut []),
+                Err(FlashRefusal::Form {
+                    opcode,
+                    address_bits: address.map_or(0, |a| a.bits),
+                    dummy_cycles,
+                    data_bytes: u32::try_from(data_out.len()).unwrap(),
+                }),
+                "{command:?}"
+            );
+        }
+        assert_eq!(content(&flash, 0, 16), [0x0F; 16]);
+        assert_eq!(status(&mut flash), 0x02, "the latch is as it was");
+    }
+
     #[test]
     fn a_read_of_no_byte_is_accepted_and_does_nothing() {
         let flash = NorFlash::with_image(vec![1, 2, 3], FlashConfig::default()).unwrap();
@@ -558,6 +1343,31 @@ mod tests {
     #[test]
     fn debug_output_does_not_dump_the_image() {
         let flash = NorFlash::with_image(vec![0xAB; 4096], FlashConfig::default()).unwrap();
-        assert!(format!("{flash:?}").len() < 200);
+        assert!(format!("{flash:?}").len() < 300);
+    }
+
+    #[test]
+    fn the_journal_keeps_the_first_entries_nobody_took_and_counts_the_others() {
+        let mut journal = FlashJournal::default();
+        let entry = |start| FlashWrite::Erase {
+            start,
+            len: SECTOR_BYTES,
+        };
+        for start in 0..u32::try_from(JOURNAL_CAPACITY).unwrap() + 6 {
+            journal.record(entry(start));
+        }
+        assert_eq!(journal.dropped(), 6);
+        let taken = journal.take();
+        assert_eq!(taken.len(), JOURNAL_CAPACITY);
+        assert_eq!(
+            (taken.first(), taken.last()),
+            (Some(&entry(0)), Some(&entry(1023))),
+            "the oldest are kept"
+        );
+        // Once taken, there is room again; the count of what was lost stays.
+        journal.record(entry(7));
+        assert_eq!(journal.take(), [entry(7)]);
+        assert_eq!(journal.take(), []);
+        assert_eq!(journal.dropped(), 6);
     }
 }
````

The controller:

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -529,7 +549,7 @@ impl Peripheral<Ctx> for Spi1 {
 #[cfg(test)]
 mod tests {
     use super::*;
-    use crate::flash::{FlashConfig, NorFlash, UnmodeledCommand};
+    use crate::flash::{FlashConfig, FlashRefusal, NorFlash};
     use std::{cell::RefCell, rc::Rc};
 
     /// `CMD` bit 17, `FLASH_PE`: a flag that accompanies a command bit (see `Spi1::run`).
@@ -639,48 +659,186 @@ mod tests {
         assert_eq!(ctx.take_unsupported(), None);
     }
 
+    const CMD_WREN: u32 = 1 << 30;
+    /// `W0..W15` hold the bytes 1 to 64.
+    fn fill_data_registers(ctx: &mut Ctx, spi: &mut Spi1) {
+        for word in 0..DATA_WORDS {
+            spi.write(ctx, W0 + 4 * word, 0x0403_0201 + 0x0404_0404 * word);
+        }
+    }
+    fn flash_bytes(spi: &Spi1, offset: u32, len: usize) -> Vec<u8> {
+        let mut bytes = vec![0u8; len];
+        spi.flash().read(offset, &mut bytes);
+        bytes
+    }
+
     #[test]
-    fn program_and_erase_are_reported_by_opcode() {
+    fn both_encodings_of_an_erase_and_of_a_program_write_the_chip_once_per_write_of_cmd() {
+        // The ROM erases with `FLASH_SE | FLASH_PE` and programs 32 bytes at most (FW §A.3);
+        // ESP-IDF erases with `FLASH_SE` alone and programs up to 64 (FW §A.2).
         let ctx = &mut Ctx::default();
         let mut spi = spi1();
-        spi.write(ctx, ADDR, 0x0400_E000); // a program of 4 bytes
-        for (command_bit, opcode) in [
-            (1u32 << 25, 0x02u8), // PP
-            (1 << 24, 0x20),      // SE
-            (1 << 23, 0xD8),      // BE
-            (1 << 22, 0xC7),      // CE
-            (1 << 26, 0x01),      // WRSR
-            (1 << 31, 0x03),      // FLASH_READ
-        ] {
-            assert!(
-                spi.write(ctx, CMD, command_bit),
-                "the write is handled: it is reported, not dropped"
-            );
+        fill_data_registers(ctx, &mut spi);
+        let pattern: Vec<u8> = (1..=64u8).collect();
+        for (erase, len) in [(0x0102_0000u32, 32usize), (0x0100_0000, 64)] {
+            spi.write(ctx, CMD, CMD_WREN);
+            spi.write(ctx, ADDR, 0xE000);
+            spi.write(ctx, CMD, erase);
+            assert_eq!(flash_bytes(&spi, 0xE000, 68), vec![0xFF; 68]);
+            spi.write(ctx, CMD, CMD_WREN);
+            spi.write(ctx, ADDR, (u32::try_from(len).unwrap() << 24) | 0xE000);
+            spi.write(ctx, CMD, CMD_PP);
+            let content = flash_bytes(&spi, 0xE000, 68);
+            assert_eq!(content[..len], pattern[..len]);
+            assert!(content[len..].iter().all(|&byte| byte == 0xFF));
             assert_eq!(
-                ctx.take_unsupported(),
-                Some(Unsupported::FlashCommand { opcode })
+                spi.take_flash_writes(),
+                [
+                    FlashWrite::Erase {
+                        start: 0xE000,
+                        len: 0x1000
+                    },
+                    FlashWrite::Program {
+                        address: 0xE000,
+                        data: pattern[..len].to_vec()
+                    }
+                ],
+                "one chip operation for each write of CMD (erase {erase:#010x})"
             );
-            assert_eq!(ctx.take_unsupported(), None, "taken once");
+            spi.write(ctx, CMD, CMD_RDSR);
             assert_eq!(
-                spi.read(ctx, CMD),
+                spi.read(ctx, RD_STATUS).map(|status| status & 0xFF),
                 Some(0),
-                "no hang on an unmodelled command"
+                "the latch is clear after the operation"
             );
         }
+        assert_eq!(ctx.take_unsupported(), None);
+        assert_eq!(spi.flash().modified_sectors(), 1);
     }
 
     #[test]
-    fn unknown_user_opcode_is_reported() {
+    fn a_program_or_an_erase_without_write_enable_changes_nothing_and_is_no_refusal() {
+        // What the part does (FW §C): it ignores the command. Nothing is unfaithful here, so
+        // nothing is reported.
+        let ctx = &mut Ctx::default();
+        let image = vec![0x5A; 0x2_0000];
+        let mut spi = Spi1::new(Box::new(
+            NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap(),
+        ));
+        fill_data_registers(ctx, &mut spi);
+        spi.write(ctx, ADDR, 0x0400_0000);
+        for command in [CMD_PP, CMD_SE, CMD_BE, 1 << 22, CMD_WRSR] {
+            assert!(spi.write(ctx, CMD, command));
+            assert_eq!(spi.read(ctx, CMD), Some(0));
+        }
+        assert_eq!(flash_bytes(&spi, 0, 0x2_0000), image);
+        assert_eq!(spi.take_flash_writes(), []);
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn a_refusal_of_the_chip_is_reported_as_what_it_is_and_cmd_reads_idle() {
         let ctx = &mut Ctx::default();
         let mut spi = spi1();
-        spi.write(ctx, W0, 0xDEAD_BEEF);
+        // High-performance mode, `A3`: an opcode the chip model does not know.
+        assert!(spi.write(ctx, CMD, 1 << 19));
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashCommand { opcode: 0xA3 })
+        );
+        assert_eq!(ctx.take_unsupported(), None, "taken once");
+        assert_eq!(spi.read(ctx, CMD), Some(0), "no hang on a refused command");
+        // A program while block-protect bits are set: status 0x3C, written through the registers.
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, RD_STATUS, 0x3C);
+        spi.write(ctx, CMD, CMD_WRSR);
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, ADDR, 0x0100_9000);
+        spi.write(ctx, CMD, CMD_PP);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashProtected {
+                opcode: 0x02,
+                status: 0x3E
+            })
+        );
+        // A program through `USR` with a 4-byte address: the frame ESP-IDF uses above 16 MiB,
+        // with the 3-byte opcode.
         spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_MOSI);
-        spi.write(ctx, USER2, 0x7000_0002); // page program
+        spi.write(ctx, USER1, 31 << 26);
+        spi.write(ctx, USER2, 0x7000_0002);
         spi.write(ctx, MOSI_DLEN, 31);
         spi.write(ctx, CMD, CMD_USR);
         assert_eq!(
             ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0x02 })
+            Some(Unsupported::FlashCommandForm {
+                opcode: 0x02,
+                address_bits: 32,
+                dummy_cycles: 0,
+                data_bytes: 4
+            })
+        );
+        assert_eq!(
+            spi.take_flash_writes(),
+            [FlashWrite::Status { value: 0x3C }],
+            "only the status write was performed"
+        );
+        // An erase past the end of a 1 MiB chip.
+        let small = FlashConfig::new(1 << 20, crate::flash::JedecId::from_u24(0x9D_6014).unwrap());
+        let mut spi = Spi1::new(Box::new(NorFlash::blank(small.unwrap())));
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, ADDR, 0x10_0000);
+        spi.write(ctx, CMD, CMD_SE);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashRange {
+                opcode: 0x20,
+                address: 0x10_0000,
+                len: 0x1000
+            })
+        );
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn a_reset_of_the_controller_keeps_the_chip_and_the_journal_of_the_run() {
+        let ctx = &mut Ctx::default();
+        let mut spi = spi1();
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, W0, 0x1234_5600);
+        spi.write(ctx, ADDR, 0x0100_9000);
+        spi.write(ctx, CMD, CMD_PP);
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.reset();
+        assert_eq!(spi.read(ctx, ADDR), Some(0), "the registers are reset");
+        assert_eq!(flash_bytes(&spi, 0x9000, 1), [0x00], "the content is kept");
+        spi.write(ctx, CMD, CMD_RDSR);
+        assert_eq!(
+            spi.read(ctx, RD_STATUS),
+            Some(0x02),
+            "the chip is another component: its latch is as the firmware left it"
+        );
+        assert_eq!(
+            spi.take_flash_writes(),
+            [FlashWrite::Program {
+                address: 0x9000,
+                data: vec![0x00]
+            }]
+        );
+    }
+
+    #[test]
+    fn unknown_user_opcode_is_reported() {
+        let ctx = &mut Ctx::default();
+        let mut spi = spi1();
+        spi.write(ctx, W0, 0xDEAD_BEEF);
+        spi.write(ctx, USER, USR_COMMAND | USR_MISO);
+        spi.write(ctx, USER2, 0x7000_0035); // a second status register this part does not have
+        spi.write(ctx, MISO_DLEN, 7);
+        spi.write(ctx, CMD, CMD_USR);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashCommand { opcode: 0x35 })
         );
         assert_eq!(
             spi.read(ctx, W0),
@@ -738,7 +896,7 @@ mod tests {
             &mut self,
             command: &FlashCommand<'_>,
             data_in: &mut [u8],
-        ) -> Result<(), UnmodeledCommand> {
+        ) -> Result<Option<FlashWrite>, FlashRefusal> {
             for (byte, answer) in data_in.iter_mut().zip(0xA0u8..) {
                 *byte = answer;
             }
@@ -748,7 +906,13 @@ mod tests {
                 command.dummy_cycles,
                 command.data_out.to_vec(),
             ));
-            Ok(())
+            Ok(None)
+        }
+        fn generation(&self) -> u64 {
+            0
+        }
+        fn modified_sectors(&self) -> u32 {
+            0
         }
     }
 
@@ -1068,15 +1232,20 @@ mod tests {
             [(0x20, Some(at(0xE000)), 0, Vec::new())]
         );
         assert_eq!(spi.read(ctx, CMD), Some(0), "self-clears");
-        // On the read-only chip the erase is refused once, by its own opcode: `USER2` (0x00 in its low
-        // byte at reset) is not a second command.
-        let mut refused = spi1();
-        refused.write(ctx, CMD, ROM_SECTOR_ERASE);
+        // On the chip it is one erase: `USER2` (0x00 in its low byte at reset) is not a second
+        // command, and nothing is reported.
+        let mut chip = spi1();
+        chip.write(ctx, CMD, CMD_WREN);
+        chip.write(ctx, ADDR, 0xE000);
+        chip.write(ctx, CMD, ROM_SECTOR_ERASE);
         assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0x20 })
+            chip.take_flash_writes(),
+            [FlashWrite::Erase {
+                start: 0xE000,
+                len: 0x1000
+            }]
         );
-        assert_eq!(ctx.take_unsupported(), None, "one command, one report");
+        assert_eq!(ctx.take_unsupported(), None);
     }
 
     #[test]
@@ -1089,7 +1258,7 @@ mod tests {
         const USER_START_PE: u32 = 0x0006_0000;
         assert_eq!(USER_START_PE, CMD_USR | CMD_FLASH_PE);
         let ctx = &mut Ctx::default();
-        let mut page_program = |spi: &mut Spi1| {
+        let page_program = |ctx: &mut Ctx, spi: &mut Spi1| {
             spi.write(ctx, W0, 0x0403_0201);
             spi.write(ctx, USER, USR_COMMAND | USR_ADDR | USR_MOSI);
             spi.write(ctx, USER1, 23 << 26);
@@ -1099,19 +1268,24 @@ mod tests {
             spi.write(ctx, CMD, USER_START_PE);
         };
         let (mut spi, seen) = recording_spi1();
-        page_program(&mut spi);
+        page_program(ctx, &mut spi);
         assert_eq!(
             seen.borrow().as_slice(),
             [(0x02, Some(at(0xE000)), 0, vec![1, 2, 3, 4])],
             "one transfer, with its address and its data"
         );
-        let mut refused = spi1();
-        page_program(&mut refused);
+        // On the chip, once its latch is set: one program.
+        let mut chip = spi1();
+        chip.write(ctx, CMD, CMD_WREN);
+        page_program(ctx, &mut chip);
         assert_eq!(
-            ctx.take_unsupported(),
-            Some(Unsupported::FlashCommand { opcode: 0x02 })
+            chip.take_flash_writes(),
+            [FlashWrite::Program {
+                address: 0xE000,
+                data: vec![1, 2, 3, 4]
+            }]
         );
-        assert_eq!(ctx.take_unsupported(), None, "one command, one report");
+        assert_eq!(ctx.take_unsupported(), None);
     }
 
     #[test]
````

The diagnostics, the bus and the machine:

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -573,6 +642,56 @@ mod tests {
         }
     }
 
+    /// The three refusals of the flash chip that are not an unknown opcode.
+    const FORM: Unsupported = Unsupported::FlashCommandForm {
+        opcode: 0x02,
+        address_bits: 32,
+        dummy_cycles: 0,
+        data_bytes: 4,
+    };
+    const RANGE: Unsupported = Unsupported::FlashRange {
+        opcode: 0x20,
+        address: 0x10_0000,
+        len: 0x1000,
+    };
+    const PROTECTED: Unsupported = Unsupported::FlashProtected {
+        opcode: 0xD8,
+        status: 0x3E,
+    };
+
+    #[test]
+    fn a_refusal_of_the_flash_chip_becomes_the_unsupported_operation_of_the_same_kind() {
+        use crate::flash::FlashRefusal;
+        assert_eq!(
+            Unsupported::from(FlashRefusal::Unmodeled { opcode: 0x35 }),
+            Unsupported::FlashCommand { opcode: 0x35 }
+        );
+        assert_eq!(
+            Unsupported::from(FlashRefusal::Form {
+                opcode: 0x02,
+                address_bits: 32,
+                dummy_cycles: 0,
+                data_bytes: 4
+            }),
+            FORM
+        );
+        assert_eq!(
+            Unsupported::from(FlashRefusal::OutOfRange {
+                opcode: 0x20,
+                address: 0x10_0000,
+                len: 0x1000
+            }),
+            RANGE
+        );
+        assert_eq!(
+            Unsupported::from(FlashRefusal::Protected {
+                opcode: 0xD8,
+                status: 0x3E
+            }),
+            PROTECTED
+        );
+    }
+
     #[test]
     fn strict_mode_stops_on_the_events_that_cannot_be_right_silently() {
         for diagnostic in [
@@ -594,6 +713,9 @@ mod tests {
             unsupported(Unsupported::FlashProgramLength { addr: 0x4100_9000 }),
             unsupported(Unsupported::FlashCommandBits { cmd: 0x0300_0000 }),
             unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
+            unsupported(FORM),
+            unsupported(RANGE),
+            unsupported(PROTECTED),
             mapping(3, MappingFault::InvalidEntry),
             mapping(3, MappingFault::BeyondFlash),
         ] {
@@ -772,6 +894,18 @@ mod tests {
                 unsupported(Unsupported::FlashSuspend { sus_ctrl: 0x20 }),
                 "program/erase suspend or resume (value 0x00000020",
             ),
+            (
+                unsupported(FORM),
+                "flash command 0x02 was sent in a form the virtual flash does not know (32 address bits, 0 dummy clocks, 4 data bytes",
+            ),
+            (
+                unsupported(RANGE),
+                "flash command 0x20 addresses 4096 bytes at 0x100000, past the end of the flash",
+            ),
+            (
+                unsupported(PROTECTED),
+                "flash command 0xd8 would program or erase while block-protect bits of the flash status register are set (status 0x3e",
+            ),
             (mapping(3, MappingFault::InvalidEntry), "MMU entry 3"),
             (
                 mapping(3, MappingFault::BeyondFlash),
@@ -791,9 +925,9 @@ mod tests {
 
     #[test]
     fn a_refused_flash_command_is_described_truthfully_whatever_the_command_does() {
-        // The same text serves a program (0x02), an erase (0x20), a data read (0x03, 0x0B, 0xBB: how
-        // `esp_flash` reads NVS), deep power-down (0xB9) and an opcode no chip has (0x00).
-        for opcode in [0x02, 0x20, 0x03, 0x0B, 0xBB, 0xB9, 0x00] {
+        // The same text serves a data read (0x03, 0x0B, 0xBB: how `esp_flash` reads NVS), deep
+        // power-down (0xB9), a 4-byte-address program (0x12) and an opcode no chip has (0x00).
+        for opcode in [0x03, 0x0B, 0xBB, 0xB9, 0x12, 0x00] {
             let text = unsupported(Unsupported::FlashCommand { opcode }).to_string();
             assert!(
                 text.contains(&format!("flash command {opcode:#04x} is not modeled")),
@@ -801,13 +935,13 @@ mod tests {
             );
             assert!(
                 text.contains(
-                    "only the identification and status commands of the virtual flash are modeled, so nothing was read or changed"
+                    "the virtual flash knows the identification and status commands, the page program (0x02), the erases (0x20, 0xd8, 0x60, 0xc7) and the status write (0x01), so nothing was read or changed"
                 ),
                 "{text}"
             );
             assert!(
-                !text.contains("read-only") && !text.contains("program or erase"),
-                "a refused read is not a write: {text}"
+                !text.contains("read-only"),
+                "the flash is no longer read-only: {text}"
             );
         }
     }
````

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -755,9 +755,9 @@ mod tests {
             ..SocConfig::default()
         });
         b.set_pc(0x4038_0010);
-        b.write(cmd, Word, 1 << 24).unwrap(); // sector erase
+        b.write(cmd, Word, 1 << 19).unwrap(); // high-performance mode, which the chip does not know
         let expected = Diagnostic::Unsupported {
-            what: crate::diag::Unsupported::FlashCommand { opcode: 0x20 },
+            what: crate::diag::Unsupported::FlashCommand { opcode: 0xA3 },
             pc: 0x4038_0010,
         };
         assert_eq!(b.ctx.diag.drain(), vec![expected.clone()]);
````

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -525,8 +543,8 @@ mod tests {
     use crate::memmap::ROM_BASE;
     use crate::memmap::SRAM_DBUS_BASE;
     use crate::test_support::{
-        HANG, T0, T1, flat_rom, load_register, print_program as print, stop_watchdogs,
-        store_register, unmodeled_register, within,
+        HANG, T0, T1, flash_erase_sector, flash_program, flat_rom, load_register,
+        print_program as print, stop_watchdogs, store_register, unmodeled_register, within,
     };
     use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};
     use cpu_rv32::csr_esp32c3::{PCCR_USER, PCER_USER, PCMR_USER};
@@ -568,6 +586,45 @@ mod tests {
         );
     }
 
+    #[test]
+    fn what_a_program_writes_to_the_flash_is_in_the_journal_of_the_run_once() {
+        // An erase, then the bitmap word NVS programs twice in place (SF §B.2).
+        let mut program = flash_erase_sector(0x9000);
+        program.extend(flash_program(0x9020, &[0xFE, 0xFF, 0xFF, 0xFF]));
+        program.extend(flash_program(0x9020, &[0xFA]));
+        program.push(HANG);
+        let strict = SocConfig {
+            strict_mmio: true,
+            ..SocConfig::default()
+        };
+        let mut m = Esp32C3::with_flash(flat_rom(&program), vec![0u8; 0xA000], &strict).unwrap();
+        assert_eq!(
+            m.run_until(Cycles(400)),
+            StopReason::CycleBudget,
+            "a flash write is nothing a strict run stops on"
+        );
+        assert_eq!(
+            m.take_flash_writes(),
+            [
+                FlashWrite::Erase {
+                    start: 0x9000,
+                    len: 0x1000
+                },
+                FlashWrite::Program {
+                    address: 0x9020,
+                    data: vec![0xFE, 0xFF, 0xFF, 0xFF]
+                },
+                FlashWrite::Program {
+                    address: 0x9020,
+                    data: vec![0xFA]
+                },
+            ]
+        );
+        assert_eq!(m.take_flash_writes(), [], "taken once");
+        assert_eq!(m.flash_writes_dropped(), 0);
+        assert_eq!(m.drain_diagnostics(), []);
+    }
+
     #[test]
     fn hang_instruction_is_what_the_assembler_produces() {
         assert_eq!(jal(0, 0), HANG);
@@ -917,8 +974,9 @@ mod tests {
         assert_eq!(m.cpu_frequency().hz(), 20_000_000);
     }
 
-    /// A flash chip whose content the test can change behind the machine's back, which the real
-    /// read-only chip never does: the only way to see decoded code being dropped.
+    /// A flash chip whose content the test can change behind the machine's back, without its
+    /// generation moving, which the real chip never does: the way to see that a write to a cache
+    /// control register drops the decoded code by itself.
     #[derive(Debug)]
     struct SharedFlash(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
 
@@ -936,11 +994,17 @@ mod tests {
             &mut self,
             command: &crate::flash::FlashCommand<'_>,
             _data_in: &mut [u8],
-        ) -> Result<(), crate::flash::UnmodeledCommand> {
-            Err(crate::flash::UnmodeledCommand {
+        ) -> Result<Option<crate::flash::FlashWrite>, crate::flash::FlashRefusal> {
+            Err(crate::flash::FlashRefusal::Unmodeled {
                 opcode: command.opcode,
             })
         }
+        fn generation(&self) -> u64 {
+            0
+        }
+        fn modified_sectors(&self) -> u32 {
+            0
+        }
     }
 
     #[test]
````

The two ignored tests of the OTA layout (`tests/reference_boot.rs`): the bootloader finds no factory image, erases the sector of `otadata` and programs its 32 bytes on the first boot, and on the first boot only.

````diff
--- a/crates/soc-esp32c3/tests/reference_boot.rs
+++ b/crates/soc-esp32c3/tests/reference_boot.rs
@@ -8,10 +8,11 @@ mod common;
 
 use emu_core::Cycles;
 use sha2::{Digest, Sha256};
-use soc_esp32c3::diag::Unsupported;
-use soc_esp32c3::memmap::{ROM_BASE, ROM_SIZE, SRAM_DBUS_BASE, SRAM_SIZE};
+use soc_esp32c3::memmap::{SRAM_DBUS_BASE, SRAM_SIZE};
 use soc_esp32c3::reset::ResetCause;
-use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, RomImage, SocConfig, StopReason};
+use soc_esp32c3::{
+    ConsoleChannel, Diagnostic, Esp32C3, FlashWrite, RomImage, SocConfig, StopReason,
+};
 
 /// SHA-256 of the merged reference image (`docs/research/sp1b-test-oracle.md`, §2).
 const REFERENCE_SHA256: &str = "fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2";
@@ -128,8 +129,8 @@ const SECOND_NS: u64 = 1_000_000_000;
 const X4_STOCK_TABLE: &[u8] = include_bytes!("../../../tests/fixtures/partitions/x4-stock.bin");
 /// Flash offset of the partition table (ESP-IDF default, `CONFIG_PARTITION_TABLE_OFFSET`).
 const PARTITION_TABLE_OFFSET: usize = 0x8000;
-/// The instruction-bus addresses of the mask ROM (report BC §A.1).
-const MASK_ROM: std::ops::Range<u32> = ROM_BASE..ROM_BASE + ROM_SIZE;
+/// Flash offset of the `otadata` partition in the stock X4 table, and of its first entry.
+const OTADATA_OFFSET: u32 = 0xE000;
 
 /// The ESP32-C3 (rev 3) mask ROM named by `INKEMU_ROM_ELF_DIR`; every failure says why and what to do.
 fn rom() -> RomImage {
@@ -188,6 +189,8 @@ fn mask_timestamp(line: &str) -> String {
 struct Run {
     console: String,
     diagnostics: Vec<Diagnostic>,
+    /// Every program, erase and status write the flash chip performed, in order.
+    flash_writes: Vec<FlashWrite>,
     stop: StopReason,
     trace_hash: Option<u64>,
     resets: u64,
@@ -234,6 +237,7 @@ fn run(config: SocConfig, done: impl Fn(&str) -> bool) -> Run {
 fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic]) -> bool) -> Run {
     let mut machine = Esp32C3::with_flash(rom(), image, &config).unwrap();
     let (mut uart0, mut diagnostics) = (Vec::new(), Vec::new());
+    let mut flash_writes = Vec::new();
     let mut stop = StopReason::CycleBudget;
     let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
     // `Asleep` is the CPU waiting for its next interrupt: device time goes on, the run too.
@@ -246,7 +250,13 @@ fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic
         uart0.extend(machine.take_console(ConsoleChannel::Uart0));
         let _ = machine.take_console(ConsoleChannel::UsbSerialJtag);
         diagnostics.extend(machine.drain_diagnostics());
+        flash_writes.extend(machine.take_flash_writes());
     }
+    assert_eq!(
+        machine.flash_writes_dropped(),
+        0,
+        "the journal was taken at every slice: it never filled"
+    );
     assert!(
         machine.device_time_nanos() < DEVICE_TIME_BUDGET_NS || done(&text(&uart0), &diagnostics),
         "device time ran out ({} ns) after {} CPU cycles before the run was done: the CPU sleeps and nothing wakes it?\nconsole:\n{}",
@@ -259,6 +269,7 @@ fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic
     Run {
         console: text(&uart0),
         diagnostics,
+        flash_writes,
         stop,
         trace_hash: machine.trace_hash(),
         resets: machine.resets(),
@@ -458,69 +469,106 @@ fn reference_image_behind_the_stock_x4_table() -> Vec<u8> {
     image
 }
 
+/// What the first boot of an OTA layout with a blank `otadata` writes (report FW §E, first row;
+/// SF §B.1): one sector erase, then one program of the entry — sequence 1, a blank label, state
+/// valid (2), CRC.
+fn the_otadata_update() -> [FlashWrite; 2] {
+    let mut entry = vec![0xFF; 32];
+    entry[..4].copy_from_slice(&[1, 0, 0, 0]);
+    entry[24..28].copy_from_slice(&[2, 0, 0, 0]);
+    entry[28..].copy_from_slice(&[0x9A, 0x98, 0x43, 0x47]);
+    [
+        FlashWrite::Erase {
+            start: OTADATA_OFFSET,
+            len: 0x1000,
+        },
+        FlashWrite::Program {
+            address: OTADATA_OFFSET,
+            data: entry,
+        },
+    ]
+}
+
+/// Asserts that `expected` are lines of `console`, in that order.
+fn assert_lines_in_order(console: &str, expected: &[&str]) {
+    let lines: Vec<String> = console.lines().map(mask_timestamp).collect();
+    let mut from = 0;
+    for expected in expected {
+        let at = lines[from..]
+            .iter()
+            .position(|line| line == expected)
+            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{lines:#?}"));
+        from += at + 1;
+    }
+}
+
+/// How many lines of `console` are `line`, timestamps masked.
+fn count_lines(console: &str, line: &str) -> usize {
+    console
+        .lines()
+        .map(mask_timestamp)
+        .filter(|found| found == line)
+        .count()
+}
+
+const NO_FACTORY_IMAGE: &str = "I (#) boot: No factory image, trying OTA 0";
+const OTADATA_SET: &str = "I (#) boot: Set actual ota_seq=1 in otadata[0]";
+
 /// Spec §5.1: "Reset → ROM → bootloader → table de partitions → `otadata` → application". The
 /// reference image has a factory application; the target device has none. Behind the stock X4
 /// table the unmodified bootloader finds a blank `otadata`, falls back to OTA slot 0 and records
-/// that choice: the mask ROM's sector-erase and page-program functions run. The flash is still
-/// read-only (plan SP1b-3b makes it writable), so both are refused, each once and by its own
-/// opcode, and nothing else is reported before the application says hello.
+/// that choice with the mask ROM's sector erase and page program. The chip takes both: the
+/// operations are counted, the bytes are the measured ones, and the second boot, which reads
+/// `otadata` through the cache window, finds the entry valid and writes nothing.
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
-fn the_stock_x4_ota_layout_boots_and_only_the_otadata_update_is_refused() {
-    let boot = run_image(
+fn the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only() {
+    let life = run_image(
         reference_image_behind_the_stock_x4_table(),
         SocConfig::default(),
-        |console, _| console.contains("Hello world!\n"),
+        |console, _| application_starts(console) == 2,
     );
     assert!(
-        boot.console.starts_with(ROM_LINES),
+        life.console.starts_with(ROM_LINES),
         "ROM part:\n{}",
-        boot.console
+        life.console
     );
-    let lines: Vec<String> = boot.console.lines().map(mask_timestamp).collect();
-    let mut from = 0;
-    for expected in [
-        "I (#) boot: No factory image, trying OTA 0",
-        "I (#) boot: Loaded app from partition at offset 0x10000",
-        "I (#) boot: Set actual ota_seq=1 in otadata[0]",
-        FIRST_APPLICATION_LINE,
-        "Hello world!",
-    ] {
-        let at = lines[from..]
-            .iter()
-            .position(|line| line == expected)
-            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{lines:#?}"));
-        from += at + 1;
-    }
-    let reported: Vec<(Unsupported, bool)> = boot
-        .diagnostics
-        .iter()
-        .map(|diagnostic| match diagnostic {
-            Diagnostic::Unsupported { what, pc } => (*what, MASK_ROM.contains(pc)),
-            other => panic!("unexpected diagnostic: {other}"),
-        })
-        .collect();
-    assert_eq!(
-        reported,
-        [
-            (Unsupported::FlashCommand { opcode: 0x20 }, true),
-            (Unsupported::FlashCommand { opcode: 0x02 }, true),
+    assert_lines_in_order(
+        &life.console,
+        &[
+            NO_FACTORY_IMAGE,
+            "I (#) boot: Loaded app from partition at offset 0x10000",
+            OTADATA_SET,
+            FIRST_APPLICATION_LINE,
+            "Hello world!",
+            "Restarting now.",
+            "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "I (#) boot: Loaded app from partition at offset 0x10000",
+            FIRST_APPLICATION_LINE,
         ],
-        "exactly the sector erase and the page program of the otadata update, each run by the mask ROM"
     );
+    // SF §B.3: both lines are printed on the first boot only. A second boot that printed them
+    // again would not have read back what the first one wrote.
+    assert_eq!(count_lines(&life.console, NO_FACTORY_IMAGE), 1);
+    assert_eq!(count_lines(&life.console, OTADATA_SET), 1);
+    assert_eq!(
+        life.flash_writes,
+        the_otadata_update(),
+        "two boots, one erase and one program: the update of the first boot"
+    );
+    assert_only_the_restart_is_reported(&life.diagnostics);
     assert!(
-        matches!(boot.stop, StopReason::CycleBudget | StopReason::Asleep),
+        matches!(life.stop, StopReason::CycleBudget | StopReason::Asleep),
         "nothing stops a lenient run: {:?}",
-        boot.stop
+        life.stop
     );
 }
 
-/// The same layout in strict mode: the run stops on the first thing the model refuses, the sector
-/// erase with which the bootloader starts its `otadata` update, and the stop names the operation
-/// and the instruction. Nothing was reported before it, and the application did not start.
+/// The same layout in strict mode: the `otadata` update is nothing the model refuses any more.
+/// The run crosses it without a stop and without a single report, and the application starts.
 #[test]
 #[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
-fn a_strict_run_of_the_stock_x4_ota_layout_stops_at_the_first_flash_command_it_refuses() {
+fn a_strict_run_of_the_stock_x4_ota_layout_writes_otadata_and_reaches_the_application() {
     let strict = SocConfig {
         strict_mmio: true,
         ..SocConfig::default()
@@ -530,47 +578,23 @@ fn a_strict_run_of_the_stock_x4_ota_layout_stops_at_the_first_flash_command_it_r
         strict,
         |console, _| console.contains("Hello world!\n"),
     );
-    let StopReason::StrictMmio(violation) = &boot.stop else {
-        panic!(
-            "a strict run of this layout must stop, got {:?}\nconsole:\n{}",
-            boot.stop, boot.console
-        );
-    };
-    let Diagnostic::Unsupported {
-        what: Unsupported::FlashCommand { opcode: 0x20 },
-        pc,
-    } = violation
-    else {
-        panic!("the stop is not the sector erase: {violation}");
-    };
-    assert!(
-        MASK_ROM.contains(pc),
-        "the erase is run by the mask ROM: pc {pc:#010x}"
-    );
-    let text = violation.to_string();
-    assert!(
-        text.contains("flash command 0x20 is not modeled") && text.contains(&format!("{pc:#010x}")),
-        "the stop names the operation and the instruction: {text}"
-    );
-    assert_eq!(
-        boot.diagnostics,
-        std::slice::from_ref(violation),
-        "nothing was reported before the stop, and the stop is in the journal once"
-    );
     assert!(
-        boot.console
-            .lines()
-            .map(mask_timestamp)
-            .any(|line| line == "I (#) boot: No factory image, trying OTA 0"),
-        "the bootloader had chosen the OTA slot:\n{}",
+        matches!(boot.stop, StopReason::CycleBudget | StopReason::Asleep),
+        "a strict run of this layout does not stop: {:?}\nconsole:\n{}",
+        boot.stop,
         boot.console
     );
-    assert_eq!(
-        application_starts(&boot.console),
-        0,
-        "the run stopped in the bootloader:\n{}",
-        boot.console
+    assert_eq!(boot.diagnostics, [], "nothing unmodelled, no exception");
+    assert_lines_in_order(
+        &boot.console,
+        &[
+            NO_FACTORY_IMAGE,
+            OTADATA_SET,
+            FIRST_APPLICATION_LINE,
+            "Hello world!",
+        ],
     );
+    assert_eq!(boot.flash_writes, the_otadata_update());
     assert_eq!(boot.resets, 0);
 }
 
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib > /tmp/t3-red.log 2>&1; echo "exit=$?"; grep -m4 "^error" /tmp/t3-red.log
```

Expected: `exit=101`; the crate does not compile: ``error[E0432]: unresolved import `crate::flash::FlashRefusal` ``, ``method `generation` is not a member of trait `FlashDevice` ``, ``cannot find type `FlashWrite` in this scope``.

- [ ] **Step 3: The chip**

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -1,9 +1,20 @@
-//! The SPI NOR flash chip behind the MSPI controllers. Read-only in this plan (decision D3):
-//! the commands that would change its content are not modelled and are reported by their opcode.
+//! The SPI NOR flash chip behind the MSPI controllers: its content, its status register, and the
+//! commands the boot chain and an ESP-IDF application send it (reports FW §C and §G, SF §B to §D).
+//!
+//! A program or an erase completes inside the command that asks for it: the write-in-progress bit
+//! is never read set (FW §A.5: every measured poll ends on its first read). The durations of a
+//! real part (FW §A.6) belong to the plan that calibrates time.
 use std::fmt;
 
 /// An erased NOR flash byte.
 pub const ERASED: u8 = 0xFF;
+/// The unit of a sector erase, and of the "modified" state of the chip.
+pub const SECTOR_BYTES: u32 = 4 << 10;
+/// The unit of a block erase (`D8`): 64 KiB for the ROM and for ESP-IDF alike (FW §A.1).
+pub const BLOCK_BYTES: u32 = 64 << 10;
+/// The unit of a page program: a program that reaches the end of its page wraps to its start
+/// (FW §C).
+pub const PAGE_BYTES: u32 = 256;
 /// One mebibyte, the unit device profiles write flash sizes in.
 pub const MIB: u32 = 1 << 20;
 pub const MIN_FLASH_BYTES: u32 = MIB;
@@ -23,8 +34,30 @@ const OP_RESUME: u8 = 0x7A;
 /// Write enable and write disable (ESP-IDF `spi_flash_defs.h`; BC §D.2 item 6).
 const OP_WREN: u8 = 0x06;
 const OP_WRDI: u8 = 0x04;
-/// Status register 1: bit 0 write-in-progress (always clear: nothing ever writes), bit 1 write-enable latch.
+/// Write the status register: one data byte (FW §C; ISSI IS25LP128F §8).
+const OP_WRSR: u8 = 0x01;
+/// Page program (FW §A.2, §A.3: the only opcode a measured boot writes data with).
+const OP_PP: u8 = 0x02;
+/// Sector erase, 4 KiB (FW §A.2, §A.3).
+const OP_SE: u8 = 0x20;
+/// Block erase, 64 KiB: what ESP-IDF sends for a step of an erase that covers a whole block
+/// (`IDF:spi_flash/esp_flash_api.c:728-742`; SF §D), and the ROM's `esp_rom_spiflash_erase_block`.
+const OP_BE: u8 = 0xD8;
+/// Chip erase: both opcodes of the datasheet. The ROM's `SPI_chip_erase` is their only sender
+/// (FW §A.1, §A.3); ESP-IDF erases a chip block by block.
+const OP_CE: u8 = 0xC7;
+const OP_CE_ALT: u8 = 0x60;
+/// Status register (FW §C): bit 0 write-in-progress (never set here), bit 1 the write-enable
+/// latch, bits 5:2 block protection `BP3..BP0`, bit 6 quad enable, bit 7 `SRWD`.
 const STATUS_WEL: u8 = 1 << 1;
+const STATUS_BP: u8 = 0b0011_1100;
+/// The bits a status write stores: `SRWD`, `QE` and `BP3..BP0`.
+const STATUS_WRITABLE: u8 = 0b1111_1100;
+/// The address length of every command that takes an address: three bytes. The 4-byte-address
+/// opcodes ESP-IDF sends above 16 MiB (`12`, `21`, `DC`: FW §A.2) are other opcodes, not modelled.
+const ADDRESS_BITS: u32 = 24;
+/// One bit per sector of the largest chip.
+const SECTOR_WORDS: usize = (MAX_FLASH_BYTES / SECTOR_BYTES / u64::BITS) as usize;
 /// Largest exponent `1 << capacity` fits a `u32`.
 const MAX_CAPACITY_EXPONENT: u8 = 31;
 
@@ -190,10 +223,72 @@ pub struct FlashCommand<'a> {
     pub data_out: &'a [u8],
 }
 
-/// The chip model does not implement this opcode: nothing happened.
+/// Why the chip did not run a command: nothing was read and nothing changed.
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
-pub struct UnmodeledCommand {
-    pub opcode: u8,
+pub enum FlashRefusal {
+    /// The model does not implement this opcode.
+    Unmodeled { opcode: u8 },
+    /// The opcode is known, but not sent like this: an address that is not 24 bits long (0 when
+    /// there is none), dummy clocks the command does not take, or a number of data bytes it cannot
+    /// carry. What a chip does with such a frame is not modelled.
+    Form {
+        opcode: u8,
+        address_bits: u32,
+        dummy_cycles: u32,
+        data_bytes: u32,
+    },
+    /// The command addresses a byte past the end of the chip. A real part wraps, or ignores the
+    /// address bits it does not have (FW §C); the model does not guess which.
+    OutOfRange { opcode: u8, address: u32, len: u32 },
+    /// A program or an erase while a block-protect bit of the status register is set. Which
+    /// blocks the bits protect depends on the part, and the part of a retail device is not
+    /// established (FW §C, §O.2): no protection map is modelled.
+    Protected { opcode: u8, status: u8 },
+}
+
+/// One operation that wrote the chip: an entry of the journal of a run.
+#[derive(Clone, Debug, PartialEq, Eq)]
+pub enum FlashWrite {
+    /// `data` was programmed from `address` on: each byte could only clear bits of the content
+    /// (`new = old AND data`), and the address wrapped at the end of its 256-byte page.
+    Program { address: u32, data: Vec<u8> },
+    /// The `len` bytes from `start` were set to `0xFF`: a sector, a block or the whole chip.
+    Erase { start: u32, len: u32 },
+    /// The status register received `value` (its writable bits).
+    Status { value: u8 },
+}
+
+/// The most entries a [`FlashJournal`] holds between two calls of [`FlashJournal::take`].
+pub const JOURNAL_CAPACITY: usize = 1024;
+
+/// The operations that wrote the chip during a run, oldest first. The firmware decides how many
+/// there are: the journal keeps the first [`JOURNAL_CAPACITY`] that nobody took and counts the
+/// others, so that its memory does not grow with what the firmware does.
+#[derive(Clone, Debug, Default)]
+pub struct FlashJournal {
+    writes: Vec<FlashWrite>,
+    dropped: u64,
+}
+
+impl FlashJournal {
+    pub(crate) fn record(&mut self, write: FlashWrite) {
+        if self.writes.len() < JOURNAL_CAPACITY {
+            self.writes.push(write);
+        } else {
+            self.dropped += 1;
+        }
+    }
+
+    /// The entries recorded since the last call, which leaves the journal empty.
+    pub fn take(&mut self) -> Vec<FlashWrite> {
+        std::mem::take(&mut self.writes)
+    }
+
+    /// How many operations were not recorded because the journal was full: since power-on.
+    #[must_use]
+    pub const fn dropped(&self) -> u64 {
+        self.dropped
+    }
 }
 
 /// The flash chip as its two users see it: the cache (content) and the SPI1 controller (commands).
@@ -201,23 +296,37 @@ pub trait FlashDevice: fmt::Debug {
     fn size(&self) -> u32;
     /// Content at `offset`; bytes the image does not hold, or past the chip, read erased.
     fn read(&self, offset: u32, out: &mut [u8]);
-    /// Runs one transaction; `data_in` receives the bytes the chip clocks out.
+    /// Runs one transaction; `data_in` receives the bytes the chip clocks out. `Ok(Some(_))`
+    /// says what a program, an erase or a status write did; `Ok(None)` is every other command,
+    /// and a write the chip ignored because its write-enable latch was clear.
     /// # Errors
-    /// [`UnmodeledCommand`] when the opcode is not implemented; `data_in` is then untouched.
+    /// [`FlashRefusal`] when the model cannot run the command; `data_in` and the chip are then
+    /// untouched.
     fn execute(
         &mut self,
         command: &FlashCommand<'_>,
         data_in: &mut [u8],
-    ) -> Result<(), UnmodeledCommand>;
+    ) -> Result<Option<FlashWrite>, FlashRefusal>;
+    /// A number that changes whenever a byte of the content does: the version of what was
+    /// decoded or read from it.
+    fn generation(&self) -> u64;
+    /// How many 4 KiB sectors hold a byte a program or an erase changed since the chip was built.
+    fn modified_sectors(&self) -> u32;
 }
 
-/// A NOR flash holding the image it was given. The image is kept as supplied: what lies past it is
-/// erased, so a blank chip costs no memory (decision D19).
+/// A NOR flash holding the image it was given. The image is kept as supplied and grows only as
+/// far as a program reaches: what lies past it is erased, so a blank chip costs no memory
+/// (decision D19).
 #[derive(Clone)]
 pub struct NorFlash {
     image: Vec<u8>,
     config: FlashConfig,
-    write_enabled: bool,
+    /// The status register, write-enable latch included. It is not part of the content: a chip
+    /// that is built starts from the factory value `0x00` (FW §C).
+    status: u8,
+    generation: u64,
+    /// One bit per 4 KiB sector whose content changed.
+    modified: [u64; SECTOR_WORDS],
 }
 
 impl fmt::Debug for NorFlash {
@@ -225,11 +334,18 @@ impl fmt::Debug for NorFlash {
         f.debug_struct("NorFlash")
             .field("image_bytes", &self.image.len())
             .field("config", &self.config)
-            .field("write_enabled", &self.write_enabled)
-            .finish()
+            .field("status", &self.status)
+            .field("generation", &self.generation)
+            .field("modified_sectors", &self.modified_sectors())
+            .finish_non_exhaustive()
     }
 }
 
+/// `offset` as an index; an offset no slice can hold indexes nothing.
+fn index(offset: u32) -> usize {
+    usize::try_from(offset).unwrap_or(usize::MAX)
+}
+
 impl NorFlash {
     /// An erased chip.
     #[must_use]
@@ -237,7 +353,9 @@ impl NorFlash {
         NorFlash {
             image: Vec::new(),
             config,
-            write_enabled: false,
+            status: 0,
+            generation: 0,
+            modified: [0; SECTOR_WORDS],
         }
     }
 
@@ -254,10 +372,191 @@ impl NorFlash {
         }
         Ok(NorFlash {
             image,
-            config,
-            write_enabled: false,
+            ..NorFlash::blank(config)
         })
     }
+
+    /// The address of a command that must carry one on 24 bits, with no dummy clock.
+    fn address_of(command: &FlashCommand<'_>) -> Result<u32, FlashRefusal> {
+        match command.address {
+            Some(address) if address.bits == ADDRESS_BITS && command.dummy_cycles == 0 => {
+                Ok(address.value)
+            }
+            _ => Err(NorFlash::form(command)),
+        }
+    }
+
+    fn form(command: &FlashCommand<'_>) -> FlashRefusal {
+        FlashRefusal::Form {
+            opcode: command.opcode,
+            address_bits: command.address.map_or(0, |address| address.bits),
+            dummy_cycles: command.dummy_cycles,
+            data_bytes: u32::try_from(command.data_out.len()).unwrap_or(u32::MAX),
+        }
+    }
+
+    /// Refuses a write the model cannot place or cannot judge: past the end of the chip, or
+    /// under a protection it has no map of. Called once the latch is known to be set: a chip
+    /// ignores a write without it, wherever it points.
+    fn writable(&self, opcode: u8, address: u32, len: u32) -> Result<(), FlashRefusal> {
+        if address >= self.config.size {
+            return Err(FlashRefusal::OutOfRange {
+                opcode,
+                address,
+                len,
+            });
+        }
+        if self.status & STATUS_BP != 0 {
+            return Err(FlashRefusal::Protected {
+                opcode,
+                status: self.status,
+            });
+        }
+        Ok(())
+    }
+
+    /// Records that the sector holding `offset` changed.
+    fn mark_modified(&mut self, offset: u32) {
+        let sector = index(offset / SECTOR_BYTES);
+        if let Some(word) = self.modified.get_mut(sector / 64) {
+            *word |= 1 << (sector % 64);
+        }
+        self.generation += 1;
+    }
+
+    /// `02`: ANDs the data into the page of the address (FW §C: a program only clears bits),
+    /// wrapping at the end of the page; of more than a page of data the last 256 bytes are kept.
+    fn program(&mut self, command: &FlashCommand<'_>) -> Result<Option<FlashWrite>, FlashRefusal> {
+        let address = NorFlash::address_of(command)?;
+        // Of more than a page of data the chip keeps the last 256 bytes (FW §C): the first
+        // `skip` bytes are dropped, and the loop below is bounded by `PAGE_BYTES` whatever the
+        // caller sends. The SPI1 controller sends 64 bytes at most.
+        let skip = command.data_out.len().saturating_sub(index(PAGE_BYTES));
+        let data = &command.data_out[skip..];
+        if data.is_empty() {
+            return Err(NorFlash::form(command));
+        }
+        if self.status & STATUS_WEL == 0 {
+            return Ok(None);
+        }
+        let len = u32::try_from(data.len()).unwrap_or(PAGE_BYTES);
+        self.writable(command.opcode, address, len)?;
+        let page = address & !(PAGE_BYTES - 1);
+        // Where the first byte kept lands: the page counter after the bytes that were dropped.
+        let dropped = u32::try_from(skip % index(PAGE_BYTES)).unwrap_or(0);
+        let first = (address - page + dropped) % PAGE_BYTES;
+        // The image grows to the end of the page it is programmed in, never past the chip:
+        // `writable` checked that the page starts inside it, and the size is a multiple of a page.
+        let page_end = index(page + PAGE_BYTES);
+        if self.image.len() < page_end {
+            self.image.resize(page_end, ERASED);
+        }
+        let mut changed = false;
+        for (position, &byte) in (first..).zip(data) {
+            let at = index(page + position % PAGE_BYTES);
+            if let Some(cell) = self.image.get_mut(at) {
+                let programmed = *cell & byte;
+                changed |= programmed != *cell;
+                *cell = programmed;
+            }
+        }
+        if changed {
+            self.mark_modified(page);
+        }
+        // "After the completion of any Write Non-Volatile Register, Program and Erase operation"
+        // the latch is clear (FW §C); ESP-IDF takes a latch still set for a command the chip
+        // did not accept (`IDF:spi_flash/spi_flash_chip_generic.c:462-468`; FW key result 5).
+        self.status &= !STATUS_WEL;
+        Ok(Some(FlashWrite::Program {
+            address: page + first,
+            data: data.to_vec(),
+        }))
+    }
+
+    /// Sets `len` bytes from `start` to `0xFF` and records the sectors whose content changed.
+    /// `start` and `len` are multiples of a sector.
+    fn fill_erased(&mut self, start: u32, len: u32) {
+        // What lies past the image is erased already. The loop is bounded by the number of
+        // sectors of the chip, 4096 at most.
+        let end = index(start.saturating_add(len)).min(self.image.len());
+        let mut sector = start;
+        while index(sector) < end {
+            let next = index(sector + SECTOR_BYTES).min(end);
+            if let Some(bytes) = self.image.get_mut(index(sector)..next)
+                && bytes.iter().any(|&byte| byte != ERASED)
+            {
+                bytes.fill(ERASED);
+                self.mark_modified(sector);
+            }
+            sector += SECTOR_BYTES;
+        }
+    }
+
+    /// `20` and `D8`: erases the sector or the block that holds the address, whose low bits are
+    /// ignored (FW §G).
+    fn erase(
+        &mut self,
+        command: &FlashCommand<'_>,
+        unit: u32,
+    ) -> Result<Option<FlashWrite>, FlashRefusal> {
+        let address = NorFlash::address_of(command)?;
+        if !command.data_out.is_empty() {
+            return Err(NorFlash::form(command));
+        }
+        if self.status & STATUS_WEL == 0 {
+            return Ok(None);
+        }
+        self.writable(command.opcode, address, unit)?;
+        let start = address & !(unit - 1);
+        self.fill_erased(start, unit);
+        self.status &= !STATUS_WEL;
+        Ok(Some(FlashWrite::Erase { start, len: unit }))
+    }
+
+    /// `C7` and `60`: erases the whole chip.
+    fn erase_chip(
+        &mut self,
+        command: &FlashCommand<'_>,
+    ) -> Result<Option<FlashWrite>, FlashRefusal> {
+        if command.address.is_some() || command.dummy_cycles != 0 || !command.data_out.is_empty() {
+            return Err(NorFlash::form(command));
+        }
+        if self.status & STATUS_WEL == 0 {
+            return Ok(None);
+        }
+        self.writable(command.opcode, 0, self.config.size)?;
+        self.fill_erased(0, self.config.size);
+        self.image.clear();
+        self.status &= !STATUS_WEL;
+        Ok(Some(FlashWrite::Erase {
+            start: 0,
+            len: self.config.size,
+        }))
+    }
+
+    /// `01`: stores the writable bits of the one status byte this part has (FW §C: no status
+    /// register 2). The stored bits last as long as the chip model: they are not in an image.
+    ///
+    /// ASSUMPTION: `SRWD` is stored and locks nothing. On the part it makes the register
+    /// read-only while the `WP#` pin is low, and the level of that pin on a device is not
+    /// established; the pin model of the board (SP2), or a schematic, settles it.
+    fn write_status(
+        &mut self,
+        command: &FlashCommand<'_>,
+    ) -> Result<Option<FlashWrite>, FlashRefusal> {
+        let [value] = *command.data_out else {
+            return Err(NorFlash::form(command));
+        };
+        if command.address.is_some() || command.dummy_cycles != 0 {
+            return Err(NorFlash::form(command));
+        }
+        if self.status & STATUS_WEL == 0 {
+            return Ok(None);
+        }
+        // Storing the byte without its latch bit is the latch cleared at completion.
+        self.status = value & STATUS_WRITABLE;
+        Ok(Some(FlashWrite::Status { value: self.status }))
+    }
 }
 
 impl FlashDevice for NorFlash {
@@ -265,6 +564,14 @@ impl FlashDevice for NorFlash {
         self.config.size
     }
 
+    fn generation(&self) -> u64 {
+        self.generation
+    }
+
+    fn modified_sectors(&self) -> u32 {
+        self.modified.iter().map(|word| word.count_ones()).sum()
+    }
+
     fn read(&self, offset: u32, out: &mut [u8]) {
         let start = usize::try_from(offset).unwrap_or(usize::MAX);
         for (i, byte) in out.iter_mut().enumerate() {
@@ -280,7 +587,7 @@ impl FlashDevice for NorFlash {
         &mut self,
         command: &FlashCommand<'_>,
         data_in: &mut [u8],
-    ) -> Result<(), UnmodeledCommand> {
+    ) -> Result<Option<FlashWrite>, FlashRefusal> {
         match command.opcode {
             // Nothing to wake up or resume: the chip is never powered down or suspended.
             OP_RES | OP_RESUME => {}
@@ -293,17 +600,22 @@ impl FlashDevice for NorFlash {
                     *byte = id;
                 }
             }
-            OP_RDSR => data_in.fill(if self.write_enabled { STATUS_WEL } else { 0 }),
+            OP_RDSR => data_in.fill(self.status),
             // ASSUMPTION: the modelled part exposes no SFDP table and answers erased bytes. The
             // bootloader only checks that byte 0x10 is not the XMC manufacturer id (BA §2.2). The
             // flash of the QEMU oracle rejects the command (`Unknown cmd 5a`), so the lockstep must
             // mask this read; the SFDP table of the real part, read on a device, settles the bytes.
             OP_SFDP => data_in.fill(ERASED),
-            OP_WREN => self.write_enabled = true,
-            OP_WRDI => self.write_enabled = false,
-            opcode => return Err(UnmodeledCommand { opcode }),
+            OP_WREN => self.status |= STATUS_WEL,
+            OP_WRDI => self.status &= !STATUS_WEL,
+            OP_WRSR => return self.write_status(command),
+            OP_PP => return self.program(command),
+            OP_SE => return self.erase(command, SECTOR_BYTES),
+            OP_BE => return self.erase(command, BLOCK_BYTES),
+            OP_CE | OP_CE_ALT => return self.erase_chip(command),
+            opcode => return Err(FlashRefusal::Unmodeled { opcode }),
         }
-        Ok(())
+        Ok(None)
     }
 }
 
````

- [ ] **Step 4: The refusals by kind**

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1,5 +1,6 @@
 //! What the emulator has to tell the user about the run: accesses it does not model, CPU
 //! exceptions, resets.
+use crate::flash::FlashRefusal;
 use crate::reset::ResetCause;
 use cpu_rv32::Exception;
 use std::collections::BTreeMap;
@@ -18,9 +19,21 @@ pub enum Direction {
 /// An operation the emulator cannot perform faithfully: it is reported, and `--strict-mmio` stops on it.
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
 pub enum Unsupported {
-    /// An SPI flash command the flash model does not implement: a program, an erase, a data read,
-    /// or an opcode it does not know (D3).
+    /// An SPI flash command the flash model does not implement: a data read, or an opcode it does
+    /// not know.
     FlashCommand { opcode: u8 },
+    /// A flash command the model knows, in a frame it does not: see
+    /// [`FlashRefusal::Form`](crate::flash::FlashRefusal::Form).
+    FlashCommandForm {
+        opcode: u8,
+        address_bits: u32,
+        dummy_cycles: u32,
+        data_bytes: u32,
+    },
+    /// A flash command that addresses a byte past the end of the chip.
+    FlashRange { opcode: u8, address: u32, len: u32 },
+    /// A program or an erase while a block-protect bit of the flash status register is set.
+    FlashProtected { opcode: u8, status: u8 },
     /// An SPI1 user transfer whose command phase is not the 8-bit one the controller model
     /// implements: no command phase at all (`None`), or a command of `command_bits` bits.
     FlashTransfer {
@@ -55,6 +68,37 @@ pub enum Unsupported {
     UartClock { uart: u8, sclk_sel: u32 },
 }
 
+impl From<FlashRefusal> for Unsupported {
+    fn from(refusal: FlashRefusal) -> Unsupported {
+        match refusal {
+            FlashRefusal::Unmodeled { opcode } => Unsupported::FlashCommand { opcode },
+            FlashRefusal::Form {
+                opcode,
+                address_bits,
+                dummy_cycles,
+                data_bytes,
+            } => Unsupported::FlashCommandForm {
+                opcode,
+                address_bits,
+                dummy_cycles,
+                data_bytes,
+            },
+            FlashRefusal::OutOfRange {
+                opcode,
+                address,
+                len,
+            } => Unsupported::FlashRange {
+                opcode,
+                address,
+                len,
+            },
+            FlashRefusal::Protected { opcode, status } => {
+                Unsupported::FlashProtected { opcode, status }
+            }
+        }
+    }
+}
+
 impl Unsupported {
     /// The message of the diagnostic: what the instruction at `pc` asked for and why it was not
     /// done, then what the user can do.
@@ -62,7 +106,28 @@ impl Unsupported {
         match self {
             Unsupported::FlashCommand { opcode } => write!(
                 f,
-                "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): only the identification and status commands of the virtual flash are modeled, so nothing was read or changed; use firmware that sends no other flash command until SPI1 data reads and flash writes are emulated"
+                "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): the virtual flash knows the identification and status commands, the page program (0x02), the erases (0x20, 0xd8, 0x60, 0xc7) and the status write (0x01), so nothing was read or changed; use firmware that sends no other flash command until SPI1 data reads are emulated"
+            ),
+            Unsupported::FlashCommandForm {
+                opcode,
+                address_bits,
+                dummy_cycles,
+                data_bytes,
+            } => write!(
+                f,
+                "flash command {opcode:#04x} was sent in a form the virtual flash does not know ({address_bits} address bits, {dummy_cycles} dummy clocks, {data_bytes} data bytes, pc {pc:#010x}) and what a chip makes of it is not modeled, so nothing was read or changed; send the command with a 24-bit address where it takes one, and with the dummy clocks and data bytes it takes"
+            ),
+            Unsupported::FlashRange {
+                opcode,
+                address,
+                len,
+            } => write!(
+                f,
+                "flash command {opcode:#04x} addresses {len} bytes at {address:#08x}, past the end of the flash of the device (pc {pc:#010x}): a chip wraps there or ignores the address bits it lacks, and inkemu does not guess which, so nothing was read or changed; keep flash accesses inside the flash size of the device profile"
+            ),
+            Unsupported::FlashProtected { opcode, status } => write!(
+                f,
+                "flash command {opcode:#04x} would program or erase while block-protect bits of the flash status register are set (status {status:#04x}, pc {pc:#010x}): which blocks they protect depends on the flash part and is not modeled, so nothing was changed; clear the protection with a status write of 0x00 before the program or the erase"
             ),
             Unsupported::FlashTransfer {
                 command_bits,
@@ -218,6 +283,10 @@ impl Diagnostic {
                 Unsupported::FlashTransfer { command_bits, .. } => {
                     (11, command_bits.unwrap_or(0), 0)
                 }
+                // One line per opcode for each kind of refusal of the chip.
+                Unsupported::FlashCommandForm { opcode, .. } => (18, u32::from(opcode), 0),
+                Unsupported::FlashRange { opcode, .. } => (19, u32::from(opcode), 0),
+                Unsupported::FlashProtected { opcode, .. } => (20, u32::from(opcode), 0),
                 // One line per byte count, per set of command bits, and one for suspend requests.
                 Unsupported::FlashProgramLength { addr } => (15, addr >> 24, 0),
                 Unsupported::FlashCommandBits { cmd } => (16, cmd, 0),
````

- [ ] **Step 5: The controller keeps the journal; the machine hands it over**

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -4,7 +4,7 @@
 use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
 use crate::ctx::Ctx;
 use crate::diag::Unsupported;
-use crate::flash::{FlashAddress, FlashCommand, FlashDevice};
+use crate::flash::{FlashAddress, FlashCommand, FlashDevice, FlashJournal, FlashWrite};
 use emu_core::Peripheral;
 use svd_regs::esp32c3::addr;
 
@@ -220,6 +220,9 @@ impl Peripheral<Ctx> for Spi0 {
 pub struct Spi1 {
     registers: Storage,
     flash: Box<dyn FlashDevice>,
+    /// What the commands of the run wrote to the chip. It belongs to the run, not to the
+    /// controller: a reset of the controller keeps it.
+    journal: FlashJournal,
 }
 
 impl Spi1 {
@@ -228,6 +231,7 @@ impl Spi1 {
         Spi1 {
             registers: Spi1::reset_registers(),
             flash,
+            journal: FlashJournal::default(),
         }
     }
 
@@ -250,12 +254,25 @@ impl Spi1 {
         self.flash.as_ref()
     }
 
+    /// The programs, erases and status writes the chip performed since the last call, oldest
+    /// first: see [`FlashJournal`].
+    pub fn take_flash_writes(&mut self) -> Vec<FlashWrite> {
+        self.journal.take()
+    }
+
+    /// The operations the journal did not keep because it was full.
+    #[must_use]
+    pub const fn flash_writes_dropped(&self) -> u64 {
+        self.journal.dropped()
+    }
+
     fn register(&self, offset: u32) -> u32 {
         self.registers.read(offset).unwrap_or(0)
     }
 
     /// Runs `command`; `Some(bytes)` are the `data_in_len` bytes read, `None` when the chip model
-    /// does not implement the opcode (the refusal is reported through the context).
+    /// refuses the command (the refusal is reported through the context). What the command
+    /// wrote, if anything, goes to the journal.
     fn transfer(
         &mut self,
         ctx: &mut Ctx,
@@ -264,11 +281,14 @@ impl Spi1 {
     ) -> Option<Vec<u8>> {
         let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
         match self.flash.execute(command, &mut data_in) {
-            Ok(()) => Some(data_in),
-            Err(refused) => {
-                ctx.unsupported(Unsupported::FlashCommand {
-                    opcode: refused.opcode,
-                });
+            Ok(written) => {
+                if let Some(write) = written {
+                    self.journal.record(write);
+                }
+                Some(data_in)
+            }
+            Err(refusal) => {
+                ctx.unsupported(Unsupported::from(refusal));
                 None
             }
         }
````

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -4,7 +4,7 @@ use crate::clock;
 use crate::config::SocConfig;
 use crate::ctx::Event;
 use crate::diag::{Diagnostic, Unsupported};
-use crate::flash::{FlashDevice, FlashImageError, NorFlash};
+use crate::flash::{FlashDevice, FlashImageError, FlashWrite, NorFlash};
 use crate::memmap::RESET_PC;
 use crate::reset::ResetCause;
 use crate::rom::RomImage;
@@ -487,6 +487,24 @@ impl Esp32C3 {
         }
     }
 
+    /// The programs, erases and status writes the flash chip performed since the last call,
+    /// oldest first: the journal of what the run wrote, for whoever wants to show or check it.
+    ///
+    /// The journal holds at most [`JOURNAL_CAPACITY`](crate::flash::JOURNAL_CAPACITY) entries
+    /// between two calls; the operations performed while it was full are counted by
+    /// [`Esp32C3::flash_writes_dropped`]. A caller that needs every one takes them as often as
+    /// it takes the console.
+    pub fn take_flash_writes(&mut self) -> Vec<FlashWrite> {
+        self.bus.periph.spi1.take_flash_writes()
+    }
+
+    /// How many flash operations the journal did not keep since power-on, because nobody had
+    /// taken the ones before them.
+    #[must_use]
+    pub const fn flash_writes_dropped(&self) -> u64 {
+        self.bus.periph.spi1.flash_writes_dropped()
+    }
+
     /// Diagnostics seen for the first time since the last call.
     pub fn drain_diagnostics(&mut self) -> Vec<Diagnostic> {
         self.bus.ctx.diag.drain()
````

````diff
--- a/crates/soc-esp32c3/src/lib.rs
+++ b/crates/soc-esp32c3/src/lib.rs
@@ -28,6 +28,8 @@ pub mod test_support;
 pub use chip::ChipRevision;
 pub use config::SocConfig;
 pub use diag::Diagnostic;
-pub use flash::{FlashConfig, FlashConfigError, FlashDevice, FlashImageError, JedecId, NorFlash};
+pub use flash::{
+    FlashConfig, FlashConfigError, FlashDevice, FlashImageError, FlashWrite, JedecId, NorFlash,
+};
 pub use machine::{ConsoleChannel, Esp32C3, StopReason};
 pub use rom::{RomError, RomImage};
````

- [ ] **Step 6: Run the suite**

```bash
cargo test --workspace > /tmp/t3.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t3.log
/usr/bin/git grep -n "SP1b-3b" -- crates > /tmp/t3-heirs.txt; wc -l < /tmp/t3-heirs.txt
```

Expected: `exit=0`, `867`, then `0` (Task 2 left one mention, the comment of the OTA test this task rewrote).

Ignored tests (the counts do not change: two tests are renamed):

```bash
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t3-i1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 10 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware > /tmp/t3-i2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 7 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t3-i3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 2 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored > /tmp/t3-i4.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` four times. `the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only` and `a_strict_run_of_the_stock_x4_ota_layout_writes_otadata_and_reaches_the_application` are among the 7.

- [ ] **Step 7: Mutations**

Rows 1 to 17: apply the change to `flash.rs`, run `cargo test -p soc-esp32c3 --lib flash::`, see the test fail, undo.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | in `program`, remove the three lines of the check `if self.status & STATUS_WEL == 0 {` (it returns `Ok(None)`) | `a_program_needs_the_write_enable_latch_and_clears_it_when_it_completes` |
| 2 | the same check removed in `erase`, then in `erase_chip` | `an_erase_without_the_latch_changes_nothing` (both) |
| 3 | the same check removed in `write_status` | `the_status_register_stores_its_writable_bits_and_its_write_needs_and_clears_the_latch` |
| 4 | in `program`, remove `self.status &= !STATUS_WEL;` | `a_program_needs_the_write_enable_latch_and_clears_it_when_it_completes` |
| 5 | in `erase`, remove `self.status &= !STATUS_WEL;` | `a_sector_erase_and_a_block_erase_set_their_whole_unit_and_nothing_else` |
| 6 | in `erase_chip`, remove `self.status &= !STATUS_WEL;` | `a_chip_erase_erases_everything_under_either_opcode` |
| 7 | `let programmed = *cell & byte;` → `let programmed = byte;` | `a_program_only_clears_bits` |
| 8 | `index(page + position % PAGE_BYTES)` → `index(page + position)` | `a_program_wraps_inside_its_256_byte_page` |
| 9 | `let start = address & !(unit - 1);` → `let start = address;` | `a_sector_erase_and_a_block_erase_set_their_whole_unit_and_nothing_else` |
| 10 | `OP_BE => return self.erase(command, BLOCK_BYTES),` → `SECTOR_BYTES` | the same test |
| 11 | `if self.status & STATUS_BP != 0 {` → `== STATUS_BP {` | `a_program_or_an_erase_under_a_block_protect_bit_is_refused_by_name` |
| 12 | `if address >= self.config.size {` → `>` | `a_write_past_the_end_of_a_smaller_chip_is_refused_by_name` |
| 13 | in `program`, remove the condition `if changed {` around `self.mark_modified(page);` | `an_operation_that_changes_no_byte_is_performed_and_modifies_nothing` |
| 14 | in `fill_erased`, remove `&& bytes.iter().any(\|&byte\| byte != ERASED)` | the same test |
| 15 | `self.status = value & STATUS_WRITABLE;` → `self.status = value;` | `the_status_register_stores_its_writable_bits_and_its_write_needs_and_clears_the_latch` |
| 16 | `if self.writes.len() < JOURNAL_CAPACITY {` → `<=` | `the_journal_keeps_the_first_entries_nobody_took_and_counts_the_others` |
| 17 | `saturating_sub(index(PAGE_BYTES))` → `saturating_sub(command.data_out.len())` | `of_more_than_a_page_of_data_the_last_256_bytes_are_programmed` |

Row 18, in `periph/spi_mem.rs`, with `cargo test -p soc-esp32c3 --lib`: replace `if let Some(write) = written { self.journal.record(write); }` by `let _ = written;` → `machine::tests::what_a_program_writes_to_the_flash_is_in_the_journal_of_the_run_once` and `periph::spi_mem::tests::both_encodings_of_an_erase_and_of_a_program_write_the_chip_once_per_write_of_cmd` fail.

Row 19, on the real ROM and Espressif's bootloader, in `flash.rs`: `*cell = programmed;` → `let _ = programmed;` (a program stores nothing), then

```bash
cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored stock_x4 > /tmp/t3-m19.log 2>&1; echo "exit=$?"
```

Expected: `exit=101`; `the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only` fails: the bootloader writes `otadata` at the second boot too (`left: 2`, `right: 1`).

- [ ] **Step 8: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(soc): the flash chip takes programs, erases and status writes; the OTA layout writes otadata

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3
```

---
## Task 4: Data reads through SPI1: the six read commands, refused past the end of the chip

ESP-IDF reads NVS, `otadata` and the partition table through SPI1 transactions, not through the cache windows: `BB` (dual I/O), a 24-bit address, 4 dummy clocks, at most 64 bytes per transaction (SF §C). Until now the chip refused every data read by opcode. It answers the six read commands; a read it cannot place or frame is refused by name.

**Files:**
- Modify: `crates/soc-esp32c3/src/flash.rs` (`READS`, `NorFlash::read_data`)
- Modify: `crates/soc-esp32c3/src/periph/spi_mem.rs` (the comment of the user address: sourced, no longer an assumption; tests)
- Modify: `crates/soc-esp32c3/src/diag.rs` (the message of an unknown opcode)

**Interfaces:**
- Consumes: `FlashRefusal::{Form, OutOfRange}`, `FlashAddress`, `FlashDevice::read` (Tasks 2 and 3).
- Produces: `NorFlash::execute` answers `03` (0 dummy clocks), `0B` (8), `3B` (8), `BB` (4), `6B` (8), `EB` (6) with a 24-bit address and no data out, filling `data_in` from the content at that address. Another address width, another dummy count or data out → `FlashRefusal::Form`; `address >= size` or `len > size - address` → `FlashRefusal::OutOfRange`, and nothing is read. No public signature changes.

**Bounds on what the firmware writes.** The length of a read is the length of `data_in`, which the controller sizes: `Spi1::data_len` caps it at `DATA_BYTES` (64). The address is compared with the size of the chip before the content is indexed; `len > size - address` cannot overflow because `address < size` is tested first.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -882,14 +938,16 @@ mod tests {
     }
 
     #[test]
-    fn data_reads_and_opcodes_the_model_does_not_know_are_refused_by_opcode() {
-        // The data reads, and what no measured boot sends: a second status register (`31`,
-        // `35`), a 32 KiB erase (`52`: no caller in ESP-IDF, SF §C), reset (`66`, `99`),
-        // suspend (`75`), deep power-down (`B9`).
+    fn opcodes_the_model_does_not_know_are_refused_by_opcode() {
+        // What no measured boot sends: a second status register (`31`, `35`), a 32 KiB erase
+        // (`52`: no caller in ESP-IDF, SF §C), reset (`66`, `99`), suspend (`75`), deep
+        // power-down (`B9`), and the commands that take a 4-byte address (`12`, `13`, `0C`,
+        // `BC`, `21`, `DC`: ESP-IDF sends them above 16 MiB, FW §A.2).
         let mut flash = NorFlash::with_image(vec![1, 2, 3, 4], FlashConfig::default()).unwrap();
         run(&mut flash, 0x06, &mut []).unwrap();
         for opcode in [
-            0x03, 0x0B, 0xBB, 0x31, 0x35, 0x52, 0x66, 0x99, 0x75, 0xB9, 0x00, 0xFF,
+            0x31, 0x35, 0x52, 0x66, 0x99, 0x75, 0xB9, 0x12, 0x13, 0x0C, 0xBC, 0x21, 0xDC, 0x00,
+            0xFF,
         ] {
             assert_eq!(
                 run(&mut flash, opcode, &mut [0u8; 4]),
@@ -913,12 +971,148 @@ mod tests {
         let mut flash = NorFlash::blank(FlashConfig::default());
         let mut data_in = [0x5A, 0xA5, 0x3C];
         assert_eq!(
-            run(&mut flash, 0x03, &mut data_in),
-            Err(FlashRefusal::Unmodeled { opcode: 0x03 })
+            run(&mut flash, 0x35, &mut data_in),
+            Err(FlashRefusal::Unmodeled { opcode: 0x35 })
         );
         assert_eq!(data_in, [0x5A, 0xA5, 0x3C], "nothing was clocked in");
     }
 
+    /// A data read: `opcode` with a 24-bit address and `dummy_cycles`.
+    fn read_at(
+        flash: &mut NorFlash,
+        opcode: u8,
+        address: u32,
+        dummy_cycles: u32,
+        data_in: &mut [u8],
+    ) -> Done {
+        flash.execute(
+            &FlashCommand {
+                opcode,
+                address: Some(FlashAddress {
+                    value: address,
+                    bits: 24,
+                }),
+                dummy_cycles,
+                data_out: &[],
+            },
+            data_in,
+        )
+    }
+
+    #[test]
+    fn every_data_read_returns_the_content_from_its_address_with_its_own_dummy_clocks() {
+        let image: Vec<u8> = (0u8..=255).cycle().take(0x400).collect();
+        let mut flash = NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap();
+        for (opcode, dummy_cycles) in [
+            (0x03u8, 0u32),
+            (0x0B, 8),
+            (0x3B, 8),
+            (0xBB, 4),
+            (0x6B, 8),
+            (0xEB, 6),
+        ] {
+            let mut data = [0u8; 64];
+            assert_eq!(
+                read_at(&mut flash, opcode, 0x123, dummy_cycles, &mut data),
+                Ok(None),
+                "{opcode:#04x}"
+            );
+            assert_eq!(data, image[0x123..0x163], "{opcode:#04x}");
+            // Another number of dummy clocks is another frame: the model does not shift the
+            // data by guessing.
+            let mut untouched = [0xA5u8; 4];
+            assert_eq!(
+                read_at(&mut flash, opcode, 0x123, dummy_cycles + 2, &mut untouched),
+                Err(FlashRefusal::Form {
+                    opcode,
+                    address_bits: 24,
+                    dummy_cycles: dummy_cycles + 2,
+                    data_bytes: 0
+                })
+            );
+            assert_eq!(untouched, [0xA5; 4]);
+        }
+        // What the image does not hold reads erased, inside the chip.
+        let mut erased = [0u8; 4];
+        read_at(&mut flash, 0xBB, 0x3FE, 4, &mut erased).unwrap();
+        assert_eq!(erased, [0xFE, 0xFF, 0xFF, 0xFF]);
+    }
+
+    #[test]
+    fn a_read_sees_what_a_program_wrote_before_it() {
+        // SF §G item 6: on its second boot the application reads 0x9060 between two programs.
+        let mut flash = NorFlash::blank(FlashConfig::default());
+        write(&mut flash, 0x02, 0x9060, &[0x01, 0x14, 0x01]).unwrap();
+        let mut entry = [0u8; 4];
+        read_at(&mut flash, 0xBB, 0x9060, 4, &mut entry).unwrap();
+        assert_eq!(entry, [0x01, 0x14, 0x01, 0xFF]);
+    }
+
+    #[test]
+    fn a_read_that_reaches_past_the_end_of_the_chip_is_refused_and_reads_nothing() {
+        let mut flash = NorFlash::with_image(vec![0x11; 16 << 20], FlashConfig::default()).unwrap();
+        let mut last = [0u8; 2];
+        assert_eq!(read_at(&mut flash, 0xBB, 0xFF_FFFE, 4, &mut last), Ok(None));
+        assert_eq!(last, [0x11; 2], "the last two bytes of the chip");
+        let mut past = [0xA5u8; 3];
+        assert_eq!(
+            read_at(&mut flash, 0xBB, 0xFF_FFFE, 4, &mut past),
+            Err(FlashRefusal::OutOfRange {
+                opcode: 0xBB,
+                address: 0xFF_FFFE,
+                len: 3
+            }),
+            "no roll-over to address 0 is guessed"
+        );
+        assert_eq!(past, [0xA5; 3]);
+        // A smaller chip does not have the upper address bits.
+        let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
+        let mut flash = NorFlash::blank(small);
+        for address in [0x10_0000, 0xFF_FFFF] {
+            assert_eq!(
+                read_at(&mut flash, 0x03, address, 0, &mut [0u8; 1]),
+                Err(FlashRefusal::OutOfRange {
+                    opcode: 0x03,
+                    address,
+                    len: 1
+                })
+            );
+        }
+        assert_eq!(
+            read_at(&mut flash, 0x03, 0x0F_FFFF, 0, &mut [0u8; 1]),
+            Ok(None)
+        );
+    }
+
+    #[test]
+    fn a_data_read_in_a_form_the_chip_does_not_know_is_refused_by_name() {
+        let mut flash = NorFlash::with_image(vec![0x22; 64], FlashConfig::default()).unwrap();
+        let at = |bits| Some(FlashAddress { value: 0, bits });
+        // (address, data sent): no address, a 4-byte address, a 2-byte address, data after it.
+        let frames: [(Option<FlashAddress>, &[u8]); 4] =
+            [(None, &[]), (at(32), &[]), (at(16), &[]), (at(24), &[0])];
+        for (address, data_out) in frames {
+            let command = FlashCommand {
+                opcode: 0xBB,
+                address,
+                dummy_cycles: 4,
+                data_out,
+            };
+            let mut data_in = [0xA5u8; 4];
+            assert_eq!(
+                flash.execute(&command, &mut data_in),
+                Err(FlashRefusal::Form {
+                    opcode: 0xBB,
+                    address_bits: address.map_or(0, |a| a.bits),
+                    dummy_cycles: 4,
+                    data_bytes: u32::try_from(data_out.len()).unwrap(),
+                }),
+                "{command:?}"
+            );
+            assert_eq!(data_in, [0xA5; 4]);
+        }
+    }
+
     #[test]
     fn a_program_needs_the_write_enable_latch_and_clears_it_when_it_completes() {
         let mut flash = NorFlash::blank(FlashConfig::default());
````

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -1070,6 +1073,92 @@ mod tests {
         assert_eq!(spi.read(ctx, W0), Some(0xA3A2_A1A0));
         assert_eq!(spi.read(ctx, W0 + 4), Some(0x0000_A5A4));
         assert_eq!(spi.read(ctx, CMD), Some(0));
+        // On the chip it is the slow read: the content at that address.
+        let mut chip = spi_with_image((0u8..=255).cycle().take(0x200).collect());
+        chip.write(ctx, ADDR, 0x101);
+        chip.write(ctx, MISO_DLEN, 8 * 4 - 1);
+        chip.write(ctx, CMD, CMD_READ);
+        assert_eq!(chip.read(ctx, W0), Some(0x0403_0201));
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    fn spi_with_image(image: Vec<u8>) -> Spi1 {
+        Spi1::new(Box::new(
+            NorFlash::with_image(image, FlashConfig::default()).unwrap(),
+        ))
+    }
+
+    /// The registers of ESP-IDF's dual I/O read of `bytes` bytes at `address` (SF §C, measured):
+    /// `USER2 = 0x700000BB`, `USER1 = 0x5C000003` (24 address bits, 4 dummy clocks),
+    /// `USER = 0xF0000080`, `ADDR` = the flash offset, `MISO_DLEN = 8 × bytes − 1`.
+    fn dio_read(ctx: &mut Ctx, spi: &mut Spi1, address: u32, bytes: u32) {
+        spi.write(ctx, USER2, 0x7000_00BB);
+        spi.write(ctx, USER1, 0x5C00_0003);
+        spi.write(ctx, USER, 0xF000_0080);
+        spi.write(ctx, ADDR, address);
+        spi.write(ctx, MISO_DLEN, 8 * bytes - 1);
+        spi.write(ctx, CMD, CMD_USR);
+    }
+
+    #[test]
+    fn the_measured_dual_io_read_fills_the_data_registers_from_the_flash() {
+        let ctx = &mut Ctx::default();
+        let image: Vec<u8> = (0u8..=255).cycle().take(0xA000).collect();
+        let mut spi = spi_with_image(image.clone());
+        let words = |spi: &mut Spi1, ctx: &mut Ctx| -> Vec<u8> {
+            (0..DATA_WORDS)
+                .flat_map(|word| spi.read(ctx, W0 + 4 * word).unwrap().to_le_bytes())
+                .collect()
+        };
+        // 64, 32 and 4 bytes: `MISO_DLEN` 0x1FF, 0xFF and 0x1F, the three lengths NVS reads.
+        dio_read(ctx, &mut spi, 0x9000, 64);
+        assert_eq!(words(&mut spi, ctx), image[0x9000..0x9040]);
+        dio_read(ctx, &mut spi, 0x9123, 32);
+        assert_eq!(words(&mut spi, ctx)[..32], image[0x9123..0x9143]);
+        dio_read(ctx, &mut spi, 0x9020, 4);
+        assert_eq!(spi.read(ctx, W0), Some(0x2322_2120));
+        assert_eq!(ctx.take_unsupported(), None);
+        assert_eq!(spi.read(ctx, CMD), Some(0));
+        assert_eq!(spi.take_flash_writes(), [], "a read writes nothing");
+    }
+
+    #[test]
+    fn a_read_the_chip_cannot_place_or_frame_is_refused_by_name_and_clocks_nothing_in() {
+        let ctx = &mut Ctx::default();
+        let mut spi = spi_with_image(vec![0x11; 64]);
+        spi.write(ctx, W0, 0xDEAD_BEEF);
+        // 64 bytes from the last byte of the chip: 63 of them are past its end.
+        dio_read(ctx, &mut spi, 0xFF_FFFF, 64);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashRange {
+                opcode: 0xBB,
+                address: 0xFF_FFFF,
+                len: 64
+            })
+        );
+        // The same read with 8 dummy clocks, which `BB` does not take.
+        spi.write(ctx, USER2, 0x7000_00BB);
+        spi.write(ctx, USER1, 0x5C00_0007);
+        spi.write(ctx, USER, 0xF000_0080);
+        spi.write(ctx, ADDR, 0);
+        spi.write(ctx, MISO_DLEN, 31);
+        spi.write(ctx, CMD, CMD_USR);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashCommandForm {
+                opcode: 0xBB,
+                address_bits: 24,
+                dummy_cycles: 8,
+                data_bytes: 0
+            })
+        );
+        assert_eq!(ctx.take_unsupported(), None);
+        assert_eq!(
+            spi.read(ctx, W0),
+            Some(0xDEAD_BEEF),
+            "nothing was clocked in"
+        );
     }
 
     #[test]
````

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -925,9 +924,9 @@ mod tests {
 
     #[test]
     fn a_refused_flash_command_is_described_truthfully_whatever_the_command_does() {
-        // The same text serves a data read (0x03, 0x0B, 0xBB: how `esp_flash` reads NVS), deep
-        // power-down (0xB9), a 4-byte-address program (0x12) and an opcode no chip has (0x00).
-        for opcode in [0x03, 0x0B, 0xBB, 0xB9, 0x12, 0x00] {
+        // The same text serves a 4-byte-address read (0x0C) or program (0x12), deep power-down
+        // (0xB9) and an opcode no chip has (0x00).
+        for opcode in [0x0C, 0x12, 0xB9, 0x00] {
             let text = unsupported(Unsupported::FlashCommand { opcode }).to_string();
             assert!(
                 text.contains(&format!("flash command {opcode:#04x} is not modeled")),
@@ -935,7 +934,7 @@ mod tests {
             );
             assert!(
                 text.contains(
-                    "the virtual flash knows the identification and status commands, the page program (0x02), the erases (0x20, 0xd8, 0x60, 0xc7) and the status write (0x01), so nothing was read or changed"
+                    "the virtual flash knows the identification and status commands, the data reads (0x03, 0x0b, 0x3b, 0xbb, 0x6b, 0xeb), the page program (0x02), the erases (0x20, 0xd8, 0x60, 0xc7) and the status write (0x01), so nothing was read or changed; use firmware that sends only these flash commands"
                 ),
                 "{text}"
             );
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib > /tmp/t4-red.log 2>&1; echo "exit=$?"; grep "^test .* FAILED$" /tmp/t4-red.log | sort
```

Expected: `exit=101` and 8 failed tests (443 pass): `diag::tests::a_refused_flash_command_is_described_truthfully_whatever_the_command_does`; in `flash::tests`, `a_data_read_in_a_form_the_chip_does_not_know_is_refused_by_name`, `a_read_sees_what_a_program_wrote_before_it`, `a_read_that_reaches_past_the_end_of_the_chip_is_refused_and_reads_nothing`, `every_data_read_returns_the_content_from_its_address_with_its_own_dummy_clocks`; in `periph::spi_mem::tests`, `a_read_the_chip_cannot_place_or_frame_is_refused_by_name_and_clocks_nothing_in`, `the_hard_wired_read_sends_its_address_and_stores_what_the_chip_answers`, `the_measured_dual_io_read_fills_the_data_registers_from_the_flash`.

- [ ] **Step 3: The chip reads**

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -47,6 +47,19 @@ const OP_BE: u8 = 0xD8;
 /// (FW §A.1, §A.3); ESP-IDF erases a chip block by block.
 const OP_CE: u8 = 0xC7;
 const OP_CE_ALT: u8 = 0x60;
+/// The data reads, each with the dummy clocks it takes after its 24-bit address: slow read,
+/// fast read, dual output, dual I/O, quad output, quad I/O
+/// (`IDF:spi_flash/include/spi_flash/spi_flash_defs.h:29-39, 60-71`; FW §A.2). The image header
+/// chooses the one an application uses; `BB` is the one measured (DIO: FW key result 3, SF §C:
+/// `USER1 = 0x5C000003`, 24 address bits and 4 dummy clocks).
+const READS: [(u8, u32); 6] = [
+    (0x03, 0),
+    (0x0B, 8),
+    (0x3B, 8),
+    (0xBB, 4),
+    (0x6B, 8),
+    (0xEB, 6),
+];
 /// Status register (FW §C): bit 0 write-in-progress (never set here), bit 1 the write-enable
 /// latch, bits 5:2 block protection `BP3..BP0`, bit 6 quad enable, bit 7 `SRWD`.
 const STATUS_WEL: u8 = 1 << 1;
@@ -534,6 +547,46 @@ impl NorFlash {
         }))
     }
 
+    /// A data read: the content from the address on, as many bytes as the controller clocks in.
+    ///
+    /// A read that would reach past the end of the chip is refused whole: the part of the
+    /// datasheet rolls its address counter over to 0 there (FW §C), a smaller part ignores the
+    /// address bits it does not have, and neither the ROM nor ESP-IDF ever asks (both reject the
+    /// range first: FW §A.2, §A.3).
+    ///
+    /// ASSUMPTION: the quad reads (`6B`, `EB`) answer whatever the quad-enable bit of the status
+    /// register holds. On the part they need it set (FW §C), and ESP-IDF sets it before it
+    /// selects a quad mode (`IDF:spi_flash/spi_flash_chip_issi.c:39-47`); what the chip clocks
+    /// out without it is not documented. An image whose header says QIO or QOUT, read against a
+    /// device, settles it.
+    fn read_data(
+        &self,
+        command: &FlashCommand<'_>,
+        dummy_cycles: u32,
+        data_in: &mut [u8],
+    ) -> Result<Option<FlashWrite>, FlashRefusal> {
+        let address = match command.address {
+            Some(address)
+                if address.bits == ADDRESS_BITS
+                    && command.dummy_cycles == dummy_cycles
+                    && command.data_out.is_empty() =>
+            {
+                address.value
+            }
+            _ => return Err(NorFlash::form(command)),
+        };
+        let len = u32::try_from(data_in.len()).unwrap_or(u32::MAX);
+        if address >= self.config.size || len > self.config.size - address {
+            return Err(FlashRefusal::OutOfRange {
+                opcode: command.opcode,
+                address,
+                len,
+            });
+        }
+        self.read(address, data_in);
+        Ok(None)
+    }
+
     /// `01`: stores the writable bits of the one status byte this part has (FW §C: no status
     /// register 2). The stored bits last as long as the chip model: they are not in an image.
     ///
@@ -588,6 +641,9 @@ impl FlashDevice for NorFlash {
         command: &FlashCommand<'_>,
         data_in: &mut [u8],
     ) -> Result<Option<FlashWrite>, FlashRefusal> {
+        if let Some(&(_, dummy_cycles)) = READS.iter().find(|(read, _)| *read == command.opcode) {
+            return self.read_data(command, dummy_cycles, data_in);
+        }
         match command.opcode {
             // Nothing to wake up or resume: the chip is never powered down or suspended.
             OP_RES | OP_RESUME => {}
````

- [ ] **Step 4: The message, and the comment that was an assumption**

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -19,8 +19,7 @@ pub enum Direction {
 /// An operation the emulator cannot perform faithfully: it is reported, and `--strict-mmio` stops on it.
 #[derive(Clone, Copy, Debug, PartialEq, Eq)]
 pub enum Unsupported {
-    /// An SPI flash command the flash model does not implement: a data read, or an opcode it does
-    /// not know.
+    /// An SPI flash command whose opcode the flash model does not implement.
     FlashCommand { opcode: u8 },
     /// A flash command the model knows, in a frame it does not: see
     /// [`FlashRefusal::Form`](crate::flash::FlashRefusal::Form).
@@ -106,7 +105,7 @@ impl Unsupported {
         match self {
             Unsupported::FlashCommand { opcode } => write!(
                 f,
-                "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): the virtual flash knows the identification and status commands, the page program (0x02), the erases (0x20, 0xd8, 0x60, 0xc7) and the status write (0x01), so nothing was read or changed; use firmware that sends no other flash command until SPI1 data reads are emulated"
+                "flash command {opcode:#04x} is not modeled (pc {pc:#010x}): the virtual flash knows the identification and status commands, the data reads (0x03, 0x0b, 0x3b, 0xbb, 0x6b, 0xeb), the page program (0x02), the erases (0x20, 0xd8, 0x60, 0xc7) and the status write (0x01), so nothing was read or changed; use firmware that sends only these flash commands"
             ),
             Unsupported::FlashCommandForm {
                 opcode,
````

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -428,9 +428,12 @@ impl Spi1 {
         let address = (user & USR_ADDR != 0).then(|| {
             // The field can ask for up to 64 bits; the register holds 32.
             let bits = (((user1 >> ADDR_BITLEN_SHIFT) & ADDR_BITLEN_MASK) + 1).min(u32::BITS);
-            // ASSUMPTION: the address is the low `bits` bits of ADDR, unshifted (ESP-IDF
-            // `spimem_flash_ll_set_usr_address` stores it as is). Only the SFDP read uses an address
-            // on the measured paths and its answer does not depend on it; the lockstep settles it.
+            // The address is the low `bits` bits of ADDR, unshifted: ESP-IDF's
+            // `spimem_flash_ll_set_usr_address` stores it as it is whatever the bit length
+            // (`spimem_flash_ll.h:570`), and a measured 24-bit read of flash offset 0xFF0000
+            // has `ADDR = 0x00FF0000` (reports FW §A.1, §P; SF §C). 24 bits is the only length
+            // the chip takes for a data command; it refuses the others by name, so nothing
+            // rests on this rule for them.
             FlashAddress {
                 value: self.register(ADDR)
                     & u32::MAX.checked_shr(u32::BITS - bits).unwrap_or(u32::MAX),
````

- [ ] **Step 5: Run the suite**

```bash
cargo test --workspace > /tmp/t4.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t4.log
tools/ci/run-ignored-tests.sh 7 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t4-i3.log 2>&1; echo "exit=$?"
```

Expected: `exit=0`, `873`, `exit=0`.

- [ ] **Step 6: Mutations**

Each row: apply the change to `flash.rs`, run `cargo test -p soc-esp32c3 --lib`, see the test fail, undo.

| Mutation | Test that fails |
|---|---|
| `if address >= self.config.size \|\| len > self.config.size - address {` → `if address >= self.config.size {` | `flash::tests::a_read_that_reaches_past_the_end_of_the_chip_is_refused_and_reads_nothing`, `periph::spi_mem::tests::a_read_the_chip_cannot_place_or_frame_is_refused_by_name_and_clocks_nothing_in` |
| in `read_data`, remove `&& command.dummy_cycles == dummy_cycles` | `flash::tests::every_data_read_returns_the_content_from_its_address_with_its_own_dummy_clocks` |
| in `READS`, `(0xBB, 4),` → `(0xBB, 8),` | `periph::spi_mem::tests::the_measured_dual_io_read_fills_the_data_registers_from_the_flash`, `flash::tests::every_data_read_returns_the_content_from_its_address_with_its_own_dummy_clocks` |
| in `read_data`, remove `address.bits == ADDRESS_BITS &&` | `flash::tests::a_data_read_in_a_form_the_chip_does_not_know_is_refused_by_name` |
| in `read_data`, remove `&& command.data_out.is_empty()` | the same test |
| in `read_data`, `self.read(address, data_in);` → `self.read(address & !0xFF, data_in);` | `flash::tests::a_read_sees_what_a_program_wrote_before_it`, `periph::spi_mem::tests::the_hard_wired_read_sends_its_address_and_stores_what_the_chip_answers` |

- [ ] **Step 7: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(soc): data reads through SPI1: the six read commands, refused past the end of the chip

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src
```

---
## Task 5: A program or an erase drops the code decoded from the flash

The CPU keeps the instructions it decoded, keyed by `Bus::page_version`. For an address of a flash window that version was the sum of two counters (the MMU table, the cache enables), because the flash could not change. It can now: without a third term, code programmed over code that already ran would never run.

**Files:**
- Modify: `crates/soc-esp32c3/src/bus.rs` (`page_version`)
- Modify: `crates/soc-esp32c3/src/machine.rs` (tests)

**Interfaces:**
- Consumes: `FlashDevice::generation()` (Task 3), `test_support::{flash_program, flash_erase_sector}` (Task 3), `Spi1::flash()`.
- Produces: `SocBus::page_version` of an address inside a flash window is `mmu.generation() + extmem.generation() + flash.generation()`. One number for both windows: a write anywhere in the chip drops all the decoded flash code. No signature changes.

No value written by the firmware sizes anything here.

- [ ] **Step 1: Write the failing tests**

A machine runs a function from the flash window, programs (or erases) the bytes of that function, and calls it again: it must run what the flash holds now.

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -1099,6 +1099,79 @@ mod tests {
         );
     }
 
+    /// A machine whose flash holds `li a0, 3; ret` at offset 0 and whose ROM maps flash page 0
+    /// at the start of the instruction window, calls that function, stores what it returned at
+    /// the start of the SRAM, runs `between`, then calls the function again and stores the result
+    /// in the next word.
+    fn calls_flash_code_around(between: &[u32]) -> Esp32C3 {
+        use cpu_rv32::asm::jalr;
+        const A0: u8 = 10;
+        const RA: u8 = 1;
+        const T2: u8 = 7;
+        const S0: u8 = 8;
+        let image: Vec<u8> = [addi(A0, 0, 3), jalr(0, RA, 0)]
+            .into_iter()
+            .flat_map(u32::to_le_bytes)
+            .collect();
+        let mut program = vec![
+            lui(T0, crate::periph::mmu::MMU_TABLE_BASE),
+            lui(T2, crate::memmap::FLASH_IBUS_BASE),
+            lui(S0, SRAM_DBUS_BASE),
+            sw(0, T0, 0),    // entry 0 = flash page 0
+            jalr(RA, T2, 0), // decodes the function: a0 = 3
+            sw(A0, S0, 0),
+        ];
+        program.extend_from_slice(between);
+        program.extend([jalr(RA, T2, 0), sw(A0, S0, 4), HANG]);
+        Esp32C3::with_flash(flat_rom(&program), image, &SocConfig::default()).unwrap()
+    }
+
+    #[test]
+    fn code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_programmed() {
+        // A program only clears bits: `li a0, 3` becomes `li a0, 1` (immediate 0b11 to 0b01).
+        const A0: u8 = 10;
+        let mut m = calls_flash_code_around(&flash_program(0, &addi(A0, 0, 1).to_le_bytes()));
+        m.run_until(Cycles(600));
+        assert_eq!(m.drain_diagnostics(), Vec::new());
+        assert_eq!(
+            (
+                bus_word(&mut m, SRAM_DBUS_BASE),
+                bus_word(&mut m, SRAM_DBUS_BASE + 4)
+            ),
+            (3, 1),
+            "the second call runs what the flash holds now"
+        );
+    }
+
+    #[test]
+    fn code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_erased() {
+        // After the erase the function is `FF FF FF FF`, which is no instruction: the second
+        // call must fault there. Code kept decoded would return 3 a second time.
+        let mut m = calls_flash_code_around(&flash_erase_sector(0));
+        m.run_until(Cycles(600));
+        let exceptions: Vec<(Exception, u32)> = m
+            .drain_diagnostics()
+            .into_iter()
+            .filter_map(|diagnostic| match diagnostic {
+                Diagnostic::Exception { cause, pc, .. } => Some((cause, pc)),
+                _ => None,
+            })
+            .collect();
+        assert_eq!(
+            exceptions.first(),
+            Some(&(Exception::IllegalInstr, crate::memmap::FLASH_IBUS_BASE)),
+            "{exceptions:?}"
+        );
+        assert_eq!(
+            (
+                bus_word(&mut m, SRAM_DBUS_BASE),
+                bus_word(&mut m, SRAM_DBUS_BASE + 4)
+            ),
+            (3, 0),
+            "the first call returned 3; the second never returned"
+        );
+    }
+
     /// Where the tests that take traps put their vector table: a page boundary, which `lui` can
     /// load (`mtvec.BASE` only has to be aligned to 256 bytes).
     const VECTORS: u32 = ROM_BASE + 0x1000;
````

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -1078,6 +1085,52 @@ mod tests {
         );
     }
 
+    #[test]
+    fn a_program_or_an_erase_that_changes_the_flash_changes_the_version_of_both_windows() {
+        // `SPI1.CMD` bits: write enable, page program, sector erase.
+        const WREN: u32 = 1 << 30;
+        const PP: u32 = 1 << 25;
+        const SE: u32 = 1 << 24;
+        let mut b = bus_with_flash(SocConfig::default());
+        let versions = |b: &SocBus| {
+            (
+                b.page_version(FLASH_IBUS_BASE + 0x2000),
+                b.page_version(FLASH_DBUS_BASE + 0x7F_0000),
+            )
+        };
+        let spi1 = |b: &mut SocBus, register: &str, value: u32| {
+            b.write(svd_regs::esp32c3::addr("SPI1", register), Word, value)
+                .unwrap();
+        };
+        let start = versions(&b);
+        let sram = b.page_version(SRAM_IBUS_BASE);
+        // One byte, 0x00, at flash offset 0x10000, which holds 0xE9. Without the write-enable
+        // latch the chip ignores the program: nothing changed, nothing is dropped.
+        spi1(&mut b, "W0", 0);
+        spi1(&mut b, "ADDR", 0x0101_0000);
+        spi1(&mut b, "CMD", PP);
+        assert_eq!(versions(&b), start);
+        spi1(&mut b, "CMD", WREN);
+        spi1(&mut b, "CMD", PP);
+        let programmed = versions(&b);
+        assert_ne!(programmed, start, "a program changed a byte");
+        assert_eq!(programmed.0, programmed.1, "one version for both windows");
+        // The same program again changes no byte.
+        spi1(&mut b, "CMD", WREN);
+        spi1(&mut b, "CMD", PP);
+        assert_eq!(versions(&b), programmed);
+        spi1(&mut b, "CMD", WREN);
+        spi1(&mut b, "ADDR", 0x1_0000);
+        spi1(&mut b, "CMD", SE);
+        assert_ne!(versions(&b), programmed, "an erase changed a byte");
+        assert_eq!(
+            b.page_version(SRAM_IBUS_BASE),
+            sram,
+            "RAM versions are untouched"
+        );
+        assert_eq!(b.ctx.diag.drain(), Vec::new());
+    }
+
     #[test]
     fn the_memories_keep_their_content_at_every_level_of_reset() {
         // TRM §6.1.1: a reset leaves the memories as they are. The RTC memory is the case the
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib > /tmp/t5-red.log 2>&1; echo "exit=$?"; grep "^test .* FAILED$" /tmp/t5-red.log | sort
```

Expected: `exit=101` and 3 failed tests (451 pass): `bus::tests::a_program_or_an_erase_that_changes_the_flash_changes_the_version_of_both_windows`, `machine::tests::code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_erased`, `machine::tests::code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_programmed`.

- [ ] **Step 3: The third term**

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -372,14 +372,21 @@ impl Bus for SocBus {
             Some(Region::Sram(dbus_addr)) => self.sram.page_version(dbus_addr),
             Some(Region::Sram0) => self.sram0.page_version(addr),
             Some(Region::Rtc) => self.rtc.page_version(addr),
-            // Flash content cannot change (the flash is read-only, D3); what an address shows changes
-            // with the MMU table and with the cache enables. Both counters only grow, so their sum
-            // changes whenever either does (decision D6). Both restart at 0 when a core reset
-            // rebuilds the MMU and the cache registers, so a version can repeat with another
-            // mapping: that is safe only because `Hart::reset` builds a new decode cache, at
-            // every level (`Esp32C3::reset`).
+            // What an address of a flash window shows changes with the MMU table, with the cache
+            // enables and with the content of the chip: a program or an erase that changes a byte
+            // moves the generation of the chip, so the code decoded from the flash before it is
+            // decoded again (spec §4.1: the decode cache is invalidated when the flash is
+            // rewritten). The three counters only grow, so their sum changes whenever one does
+            // (decision D6). The first two restart at 0 when a core reset rebuilds the MMU and the
+            // cache registers, so a version can repeat with another mapping: that is safe only
+            // because `Hart::reset` builds a new decode cache, at every level (`Esp32C3::reset`).
+            // The version is one number for both windows, not one per flash page: a write
+            // anywhere in the chip drops all the decoded flash code, which costs a decode and
+            // can never keep a stale instruction.
             Some(Region::Flash { .. }) => {
-                self.periph.mmu.generation() + self.periph.extmem.generation()
+                self.periph.mmu.generation()
+                    + self.periph.extmem.generation()
+                    + self.periph.spi1.flash().generation()
             }
             Some(Region::Rom(_) | Region::Periph) | None => 0,
         }
````

- [ ] **Step 4: Run the suite**

```bash
cargo test --workspace > /tmp/t5.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t5.log
tools/ci/run-ignored-tests.sh 7 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t5-i3.log 2>&1; echo "exit=$?"
target/release/inkemu run --device x4 --strict-mmio --max-cycles 40000000 --trace-hash --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" > /dev/null 2> /tmp/t5-hash.log; echo "exit=$?"; grep trace-hash /tmp/t5-hash.log
```

Expected: `exit=0`, `876`, `exit=0`, `exit=0` and `trace-hash 10174cc2726c3d77` (build the binary first: `cargo build --release -p emu-cli`). The version of a page is no part of a trace: the hash does not move.

- [ ] **Step 5: Mutations**

| Mutation | Command | Test that fails |
|---|---|---|
| in `bus.rs`, remove `+ self.periph.spi1.flash().generation()` | `cargo test -p soc-esp32c3 --lib` | the three tests of Step 2 |
| in `flash.rs` (`fill_erased`), replace `self.mark_modified(sector);` by the two lines that set the bit of the sector in `self.modified` without `self.generation += 1` | `cargo test -p soc-esp32c3 --lib` | `machine::tests::code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_erased`, `bus::tests::a_program_or_an_erase_that_changes_the_flash_changes_the_version_of_both_windows` |

The reference firmware cannot tell: no measured firmware runs code it has just programmed. Tried on the code of the last task, the first mutation leaves all the ignored tests of `reference_boot` passing. The two machine tests of this task are the only proof.

- [ ] **Step 6: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(soc): a program or an erase drops the code decoded from the flash

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src
```

---
## Task 6: A window read that may differ from the device is reported

The two cache windows read the flash at every access (ADR 0002). A device reads its cache: a line filled before SPI1 programmed or erased the bytes keeps the old ones until software invalidates it. Now that the flash is written, that is a place where the emulator and a device can answer differently, and the emulator must say so instead of being silently "more right". This task keeps, per 64 KiB flash page, the one sequence that matters (read through a window, written, read again with no invalidation between), reports the last read, and revises ADR 0002.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/extmem.rs` (`PageSet`, `window_read`, `flash_written`, `invalidate`, the invalidate bit)
- Modify: `crates/soc-esp32c3/src/periph/mmu.rs` (`MmuTable::flash_page`)
- Modify: `crates/soc-esp32c3/src/periph/spi_mem.rs` (`cache_pages`, the event)
- Modify: `crates/soc-esp32c3/src/periph/mod.rs` (delivery of the two events)
- Modify: `crates/soc-esp32c3/src/ctx.rs` (`Event::{FlashWritten, CacheInvalidated}`)
- Modify: `crates/soc-esp32c3/src/diag.rs` (`Diagnostic::StaleFlashWindow`)
- Modify: `crates/soc-esp32c3/src/bus.rs` (the report on the window path)
- Modify: `crates/soc-esp32c3/src/machine.rs` (one test of Task 5 now expects the report)
- Modify: `crates/emu-cli/src/run.rs` (the journal prefix of the new diagnostic)
- Modify: `crates/emu-cli/src/main.rs`, `crates/emu-cli/src/exit.rs` (the help of `--strict-mmio` and of exit code 3 name the new stop)
- Modify: `crates/emu-cli/tests/run_cli.rs` (the test of that help)
- Modify: `docs/adr/0002-flash-model-and-cache-windows.md` (revised)
- Modify: `docs/registers-accepted-without-effect.md` (regenerated: three rows reworded)

**Interfaces:**
- Consumes: `FlashDevice::generation()`, `FlashWrite` (Task 3); `Ctx::signal(Event)`; `MmuTable::entry(index)`; `MMU_ENTRIES`, `MMU_PAGE_BYTES`; `FLASH_DBUS_BASE`, `FLASH_IBUS_BASE`, `FLASH_WINDOW_SIZE`.
- Produces:
  - `Event::FlashWritten { first_page: u32, last_page: u32 }` (posted by SPI1 when a program or an erase changed a byte) and `Event::CacheInvalidated` (posted by a write of `EXTMEM.ICACHE_SYNC_CTRL` with bit 0 set).
  - `pub(crate) fn Extmem::window_read(&mut self, page: u32) -> bool`, `flash_written(&mut self, first_page: u32, last_page: u32)`, `invalidate(&mut self, mmu: &MmuTable)`.
  - `pub fn MmuTable::flash_page(&self, index: usize) -> Option<u32>`.
  - `Diagnostic::StaleFlashWindow { vaddr: u32, page: u32, pc: u32 }`: one line per flash page (key `(21, page, 0)`), a `--strict-mmio` stop; the read returns the bytes of the flash. `inkemu run --help` says so, in the text of `--strict-mmio` and of exit code 3.
  - ADR 0002 says what is true now: the chip is written (point 4), decoded code follows the content (point 5), the window is a live view and the read that may differ is named, with the limits of a state kept per page (point 6).

**Bounds on what the firmware writes.** Three values come from the firmware. (1) The page range of a write, from an address: `flash_written` walks `first_page..=last_page.min(FLASH_PAGES - 1)`, at most 256 steps; `cache_pages` computes the last byte with `saturating_add`. (2) The invalidated range, `ICACHE_SYNC_ADDR` and up to `0x7FFFFF` lines: the arithmetic is in 64 bits and the loop is `.take(MMU_ENTRIES)`, at most 128 steps. (3) A page number from an MMU entry: `PageSet` is `[u64; 4]`; a number past 255 is in no set and changes none.

- [ ] **Step 1: Write the failing tests**

The cache state:

````diff
--- a/crates/soc-esp32c3/src/periph/extmem.rs
+++ b/crates/soc-esp32c3/src/periph/extmem.rs
@@ -563,6 +712,211 @@ mod tests {
         }
     }
 
+    /// An MMU table whose entries map flash pages: `(entry, page)` pairs.
+    fn mapping(pairs: &[(u32, u32)]) -> MmuTable {
+        let ctx = &mut Ctx::default();
+        let mut mmu = MmuTable::new();
+        for &(entry, page) in pairs {
+            assert!(mmu.write(ctx, 4 * entry, page));
+        }
+        mmu
+    }
+
+    /// Software's request, as `Cache_Invalidate_ICache_Items(addr, lines)` issues it, and the
+    /// event it posts delivered as `Peripherals::deliver` does.
+    fn invalidate(x: &mut Extmem, mmu: &MmuTable, addr: u32, lines: u32) {
+        let ctx = &mut Ctx::default();
+        assert!(x.write(ctx, reg("ICACHE_SYNC_ADDR"), addr));
+        assert!(x.write(ctx, reg("ICACHE_SYNC_SIZE"), lines));
+        assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), 1));
+        assert_eq!(ctx.take_signal(), Some(Event::CacheInvalidated));
+        assert_eq!(ctx.take_signal(), None, "one request, one event");
+        x.invalidate(mmu);
+    }
+
+    /// The lines of one 64 KiB MMU page: `Cache_Invalidate_Addr(vaddr, 0x10000)` writes `0x800`.
+    const PAGE_LINES: u32 = 0x800;
+    const PAGE: u32 = MMU_PAGE_BYTES;
+
+    #[test]
+    fn a_page_read_through_a_window_then_written_is_stale_until_the_cache_is_invalidated() {
+        let mmu = mapping(&[(3, 5)]);
+        let mut x = Extmem::new();
+        assert!(!x.window_read(5), "a first read is what the flash holds");
+        x.flash_written(5, 5);
+        assert!(
+            x.window_read(5),
+            "read, then written: a device has the line"
+        );
+        assert!(x.window_read(5), "and keeps it: the next read is stale too");
+        assert!(!x.window_read(4), "another page is untouched");
+        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, PAGE_LINES);
+        assert!(!x.window_read(5), "the line was dropped: the read is fresh");
+        // The read above filled the line again: a new write makes it stale again.
+        x.flash_written(5, 5);
+        assert!(x.window_read(5));
+    }
+
+    #[test]
+    fn a_page_written_before_any_window_read_it_is_not_stale() {
+        let mmu = mapping(&[(3, 5)]);
+        let mut x = Extmem::new();
+        x.flash_written(5, 5);
+        assert!(!x.window_read(5), "no line held the bytes of before");
+        // An invalidation forgets the read too: a write after it makes nothing stale.
+        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
+        x.flash_written(5, 5);
+        assert!(!x.window_read(5));
+    }
+
+    #[test]
+    fn a_write_marks_the_pages_of_its_range_that_were_read_and_no_other() {
+        let mut x = Extmem::new();
+        assert!(!x.window_read(2));
+        assert!(!x.window_read(4));
+        assert!(!x.window_read(5));
+        x.flash_written(1, 4); // a 64 KiB block erase and more: pages 1 to 4
+        assert!(x.window_read(2));
+        assert!(x.window_read(4));
+        assert!(!x.window_read(1), "in the range, never read");
+        assert!(!x.window_read(3), "in the range, never read");
+        assert!(!x.window_read(5), "read, past the range");
+        assert!(!x.window_read(0), "before the range");
+    }
+
+    #[test]
+    fn an_invalidation_drops_the_pages_of_the_whole_entries_it_covers_in_either_window() {
+        let mmu = mapping(&[(3, 5), (4, 6), (5, 7)]);
+        let stale = |x: &mut Extmem| [5, 6, 7].map(|page| x.window_read(page));
+        let mut x = Extmem::new();
+        let fill = |x: &mut Extmem| {
+            for page in [5, 6, 7] {
+                x.window_read(page);
+            }
+            x.flash_written(5, 7);
+        };
+        fill(&mut x);
+        assert_eq!(stale(&mut x), [true, true, true]);
+        // One entry, named through the data window, then another through the instruction window.
+        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 4 * PAGE, PAGE_LINES);
+        assert_eq!(stale(&mut x), [true, false, true]);
+        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 3 * PAGE, PAGE_LINES);
+        assert_eq!(stale(&mut x), [false, false, true]);
+        // Two entries at once.
+        fill(&mut x);
+        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, 2 * PAGE_LINES);
+        assert_eq!(stale(&mut x), [false, false, true]);
+        // A range that covers no whole entry leaves the lines it does not name: nothing is dropped.
+        fill(&mut x);
+        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE, PAGE_LINES - 1);
+        assert_eq!(stale(&mut x), [true, true, true], "one line short");
+        invalidate(&mut x, &mmu, FLASH_DBUS_BASE + 3 * PAGE + 32, PAGE_LINES);
+        assert_eq!(stale(&mut x), [true, true, true], "one line late");
+        // One line late and long enough to cover the next entry: that one is dropped.
+        invalidate(
+            &mut x,
+            &mmu,
+            FLASH_DBUS_BASE + 3 * PAGE + 32,
+            2 * PAGE_LINES,
+        );
+        assert_eq!(stale(&mut x), [true, false, true]);
+    }
+
+    #[test]
+    fn an_invalidation_of_everything_drops_every_page() {
+        // `Cache_Invalidate_ICache_All` is `Cache_Invalidate_ICache_Items(0, 0)` (CM §4.1).
+        let mmu = MmuTable::new(); // nothing mapped: "everything" does not go through the table
+        let mut x = Extmem::new();
+        for page in [0, 63, 64, 255] {
+            x.window_read(page);
+        }
+        x.flash_written(0, 255);
+        assert!(x.window_read(0) && x.window_read(63) && x.window_read(64) && x.window_read(255));
+        invalidate(&mut x, &mmu, 0, 0);
+        for page in [0, 63, 64, 255] {
+            assert!(!x.window_read(page), "{page}");
+        }
+    }
+
+    #[test]
+    fn a_request_that_names_no_line_of_a_window_drops_nothing() {
+        let mmu = mapping(&[(0, 5), (127, 5)]);
+        let mut x = Extmem::new();
+        x.window_read(5);
+        x.flash_written(5, 5);
+        for (addr, lines) in [
+            (FLASH_DBUS_BASE, 0),                              // no line, at an address
+            (0, PAGE_LINES),                                   // lines, outside the windows
+            (FLASH_DBUS_BASE - PAGE, 2 * PAGE_LINES),          // starts before the window
+            (FLASH_IBUS_BASE + FLASH_WINDOW_SIZE, PAGE_LINES), // starts past its end
+            (FLASH_DBUS_BASE + PAGE, PAGE_LINES),              // an entry that maps nothing
+            (u32::MAX, u32::MAX),
+        ] {
+            invalidate(&mut x, &mmu, addr, lines);
+            assert!(x.window_read(5), "{addr:#x} {lines:#x}");
+        }
+        // The size field has 23 bits: the largest request from the last entry ends past the
+        // window and is still followed to the end of the table, no further.
+        invalidate(&mut x, &mmu, FLASH_IBUS_BASE + 127 * PAGE, u32::MAX);
+        assert!(!x.window_read(5));
+    }
+
+    #[test]
+    fn only_the_invalidate_bit_of_the_sync_register_asks_for_an_invalidation() {
+        let ctx = &mut Ctx::default();
+        let mut x = Extmem::new();
+        for value in [0, SYNC_DONE, !SYNC_INVALIDATE] {
+            assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), value));
+            assert_eq!(ctx.take_signal(), None, "{value:#x}");
+        }
+        for value in [SYNC_INVALIDATE, 3, u32::MAX] {
+            assert!(x.write(ctx, reg("ICACHE_SYNC_CTRL"), value));
+            assert_eq!(
+                ctx.take_signal(),
+                Some(Event::CacheInvalidated),
+                "{value:#x}"
+            );
+        }
+    }
+
+    #[test]
+    fn switching_the_cache_off_and_on_keeps_its_lines() {
+        // `Cache_Suspend_ICache` and `Cache_Resume_ICache` around a flash operation write
+        // `ICACHE_CTRL` and `ICACHE_CTRL1` and invalidate nothing (CM §4.1): the lines are still
+        // there when the cache is back, stale if the operation changed their bytes.
+        let ctx = &mut Ctx::default();
+        let mut x = Extmem::new();
+        x.window_read(5);
+        x.write(ctx, reg("ICACHE_CTRL1"), 3);
+        x.write(ctx, reg("ICACHE_CTRL"), 0);
+        x.flash_written(5, 5);
+        x.write(ctx, reg("ICACHE_CTRL"), 1);
+        x.write(ctx, reg("ICACHE_CTRL1"), 0);
+        assert_eq!(ctx.take_signal(), None);
+        assert!(x.window_read(5));
+    }
+
+    #[test]
+    fn page_numbers_no_entry_can_name_are_in_no_set() {
+        let mut x = Extmem::new();
+        for page in [256, 257, 1 << 16, u32::MAX] {
+            assert!(!x.window_read(page), "{page}");
+        }
+        // A range the firmware's address made absurd is followed to page 255 and no further.
+        x.window_read(255);
+        x.flash_written(0, u32::MAX);
+        assert!(x.window_read(255));
+        for page in [256, u32::MAX] {
+            assert!(!x.window_read(page), "{page}");
+        }
+        // An empty range (first past last) marks nothing.
+        let mut x = Extmem::new();
+        x.window_read(2);
+        x.flash_written(3, 2);
+        x.flash_written(300, 400);
+        assert!(!x.window_read(2));
+    }
+
     #[test]
     fn registers_no_measured_path_touches_stay_unmodelled() {
         let ctx = &mut Ctx::default();
````

````diff
--- a/crates/soc-esp32c3/src/periph/mmu.rs
+++ b/crates/soc-esp32c3/src/periph/mmu.rs
@@ -141,6 +150,21 @@ mod tests {
         assert_eq!(mmu.entry(128), None);
     }
 
+    #[test]
+    fn the_flash_page_of_an_entry_is_its_low_byte_and_an_invalid_entry_has_none() {
+        let ctx = &mut Ctx::default();
+        let mut mmu = MmuTable::new();
+        assert_eq!(mmu.flash_page(3), None, "invalid from power-on");
+        mmu.write(ctx, 4 * 3, 0x35);
+        assert_eq!(mmu.flash_page(3), Some(0x35));
+        mmu.write(ctx, 4 * 3, 0x135);
+        assert_eq!(mmu.flash_page(3), None, "bit 8 set: the entry maps nothing");
+        mmu.write(ctx, 4 * 127, 0xFF);
+        assert_eq!(mmu.flash_page(127), Some(0xFF));
+        assert_eq!(mmu.flash_page(128), None);
+        assert_eq!(mmu.flash_page(usize::MAX), None);
+    }
+
     #[test]
     fn translation_uses_the_page_number_and_keeps_the_offset_in_the_page() {
         // CM §1: entry_id = (vaddr & 0x7FFFFF) >> 16; paddr = (entry & 0xFF) << 16 | (vaddr & 0xFFFF).
````

What SPI1 tells it:

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -737,6 +766,84 @@ mod tests {
         assert_eq!(flash_bytes(&spi, 0, 0x2_0000), image);
         assert_eq!(spi.take_flash_writes(), []);
         assert_eq!(ctx.take_unsupported(), None);
+        assert_eq!(ctx.take_signal(), None, "the cache is told of nothing");
+    }
+
+    #[test]
+    fn a_command_that_changes_the_flash_tells_the_cache_which_pages_and_no_other_does() {
+        const CMD_CE: u32 = 1 << 22;
+        let ctx = &mut Ctx::default();
+        let mut spi = Spi1::new(Box::new(
+            NorFlash::with_image(vec![0x5A; 0x4_0000], FlashConfig::default()).unwrap(),
+        ));
+        let written = |first_page, last_page| {
+            Some(Event::FlashWritten {
+                first_page,
+                last_page,
+            })
+        };
+        fill_data_registers(ctx, &mut spi);
+        // Four bytes from 0x2FFFE: they wrap in their 256-byte page, so stay in 64 KiB page 2.
+        spi.write(ctx, ADDR, 0x0402_FFFE);
+        spi.write(ctx, CMD, CMD_WREN);
+        assert_eq!(ctx.take_signal(), None, "the latch is no content");
+        spi.write(ctx, CMD, CMD_PP);
+        assert_eq!(ctx.take_signal(), written(2, 2));
+        assert_eq!(ctx.take_signal(), None, "one command, one event");
+        // The same program again changes no byte: no line of a cache can be stale of it.
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, CMD, CMD_PP);
+        assert_eq!(ctx.take_signal(), None);
+        assert_eq!(spi.take_flash_writes().len(), 2, "both are in the journal");
+        // The last sector of page 1, then the 64 KiB block that is page 3.
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, ADDR, 0x1_F000);
+        spi.write(ctx, CMD, CMD_SE);
+        assert_eq!(ctx.take_signal(), written(1, 1));
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, ADDR, 0x3_0000);
+        spi.write(ctx, CMD, CMD_BE);
+        assert_eq!(ctx.take_signal(), written(3, 3));
+        // A status write changes no content; neither does a read.
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, RD_STATUS, 0);
+        spi.write(ctx, CMD, CMD_WRSR);
+        spi.write(ctx, CMD, CMD_RDSR);
+        spi.write(ctx, CMD, CMD_RDID);
+        assert_eq!(ctx.take_signal(), None);
+        // A chip erase: every page of the chip.
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, CMD, CMD_CE);
+        let last_page = FlashConfig::default().size() / MMU_PAGE_BYTES - 1;
+        assert_eq!(ctx.take_signal(), written(0, last_page));
+        assert_eq!(ctx.take_unsupported(), None);
+    }
+
+    #[test]
+    fn the_pages_of_a_write_are_those_of_its_bytes_whatever_its_numbers() {
+        let program = |address| FlashWrite::Program {
+            address,
+            data: vec![0; 64],
+        };
+        let erase = |start, len| FlashWrite::Erase { start, len };
+        assert_eq!(cache_pages(&program(0)), Some((0, 0)));
+        assert_eq!(
+            cache_pages(&program(0xFFFF)),
+            Some((0, 0)),
+            "wraps in its page"
+        );
+        assert_eq!(cache_pages(&program(0xFF_FFC0)), Some((255, 255)));
+        assert_eq!(cache_pages(&erase(0xE000, 0x1000)), Some((0, 0)));
+        assert_eq!(cache_pages(&erase(0x1_0000, 0x1_0000)), Some((1, 1)));
+        assert_eq!(cache_pages(&erase(0xF000, 0x2000)), Some((0, 1)));
+        assert_eq!(cache_pages(&erase(0, 0x100_0000)), Some((0, 255)));
+        assert_eq!(cache_pages(&FlashWrite::Status { value: 0x40 }), None);
+        // Numbers no chip produces must not overflow.
+        assert_eq!(
+            cache_pages(&erase(u32::MAX, u32::MAX)),
+            Some((0xFFFF, 0xFFFF))
+        );
+        assert_eq!(cache_pages(&erase(5, 0)), Some((0, 0)));
     }
 
     #[test]
````

````diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -1197,6 +1202,17 @@ mod tests {
         }
         p.deliver(ctx, Event::ClockChanged);
         assert_eq!(ctx.take_signal(), None);
+        // The two events of the cache state: marking and clearing flash pages posts nothing.
+        p.deliver(
+            ctx,
+            Event::FlashWritten {
+                first_page: 0,
+                last_page: u32::MAX,
+            },
+        );
+        assert_eq!(ctx.take_signal(), None);
+        p.deliver(ctx, Event::CacheInvalidated);
+        assert_eq!(ctx.take_signal(), None);
     }
 
     #[test]
````

The bus, the diagnostic, and the machine test of Task 5 that reads back what it programmed without invalidating anything:

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -1131,6 +1148,147 @@ mod tests {
         assert_eq!(b.ctx.diag.drain(), Vec::new());
     }
 
+    fn spi1(b: &mut SocBus, register: &str, value: u32) {
+        b.write(svd_regs::esp32c3::addr("SPI1", register), Word, value)
+            .unwrap();
+    }
+    /// Programs one byte through SPI1 as the ROM and ESP-IDF do: write enable, then page program.
+    fn program_byte(b: &mut SocBus, flash_address: u32, byte: u8) {
+        spi1(b, "CMD", 1 << 30);
+        spi1(b, "W0", u32::from(byte));
+        spi1(b, "ADDR", (1 << 24) | flash_address);
+        spi1(b, "CMD", 1 << 25);
+    }
+    fn erase_sector(b: &mut SocBus, flash_address: u32) {
+        spi1(b, "CMD", 1 << 30);
+        spi1(b, "ADDR", flash_address);
+        spi1(b, "CMD", 1 << 24);
+    }
+    /// `Cache_Invalidate_ICache_Items(addr, lines)` (CM §4.1).
+    fn invalidate(b: &mut SocBus, addr: u32, lines: u32) {
+        for (register, value) in [
+            ("ICACHE_SYNC_ADDR", addr),
+            ("ICACHE_SYNC_SIZE", lines),
+            ("ICACHE_SYNC_CTRL", 1),
+        ] {
+            b.write(svd_regs::esp32c3::addr("EXTMEM", register), Word, value)
+                .unwrap();
+        }
+    }
+    fn stale(vaddr: u32, page: u32) -> Diagnostic {
+        Diagnostic::StaleFlashWindow { vaddr, page, pc: 0 }
+    }
+
+    #[test]
+    fn a_window_read_of_flash_programmed_since_it_was_read_is_reported_and_returns_the_flash() {
+        let mut b = bus_with_flash(SocConfig::default());
+        map(&mut b, 3, 1);
+        map(&mut b, 0, 0);
+        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
+        assert_eq!(load(&mut b, vaddr), 0x4F02_06E9);
+        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211);
+        program_byte(&mut b, 0x1_0000, 0x00);
+        assert_eq!(
+            b.ctx.diag.drain(),
+            Vec::new(),
+            "the write itself is faithful"
+        );
+        assert_eq!(
+            load(&mut b, vaddr),
+            0x4F02_0600,
+            "the window stays a view of the flash: the new byte is returned"
+        );
+        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 1)]);
+        // The same page through the other window and at another address: the same line, counted.
+        assert_eq!(load(&mut b, FLASH_IBUS_BASE + 0x3_FFFC), 0xA3A2_A1A0);
+        assert_eq!(b.ctx.diag.drain(), Vec::new());
+        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 2);
+        // Page 0 was read too, and not written: its reads are those of a device.
+        assert_eq!(load(&mut b, FLASH_DBUS_BASE), 0x4433_2211);
+        assert_eq!(b.ctx.diag.occurrences(&stale(0, 0)), 0);
+    }
+
+    #[test]
+    fn an_invalidation_of_the_page_ends_the_report_until_it_is_written_again() {
+        let mut b = bus_with_flash(SocConfig::default());
+        map(&mut b, 3, 1);
+        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
+        load(&mut b, vaddr);
+        program_byte(&mut b, 0x1_0000, 0x00);
+        // What `spi_flash_check_and_flush_cache` does after a write to mapped flash.
+        invalidate(&mut b, vaddr, 0x800);
+        assert_eq!(load(&mut b, vaddr), 0x4F02_0600);
+        assert_eq!(
+            b.ctx.diag.drain(),
+            Vec::new(),
+            "a device reads the flash again too"
+        );
+        // That read filled the cache again; an erase makes it stale again.
+        erase_sector(&mut b, 0x1_0000);
+        assert_eq!(load(&mut b, vaddr), 0xFFFF_FFFF);
+        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 1)]);
+        // `Cache_Invalidate_ICache_All`, as `Cache_Disable_ICache` issues it.
+        invalidate(&mut b, 0, 0);
+        load(&mut b, vaddr);
+        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 1, "no new occurrence");
+    }
+
+    #[test]
+    fn flash_written_before_a_window_read_it_or_left_as_it_was_is_not_reported() {
+        let mut b = bus_with_flash(SocConfig::default());
+        map(&mut b, 3, 1);
+        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
+        // Written first, read after: no cache line held the bytes of before.
+        program_byte(&mut b, 0x1_0000, 0xE8);
+        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
+        // Read, then a program that changes no byte (0xE8 AND 0xE8).
+        program_byte(&mut b, 0x1_0000, 0xE8);
+        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
+        // Read, then a write to a page no window read.
+        program_byte(&mut b, 0x2_0000, 0x00);
+        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
+        // Switching the cache off and on is no invalidation and no write.
+        for (register, value) in [("ICACHE_CTRL", 0), ("ICACHE_CTRL", 1)] {
+            b.write(svd_regs::esp32c3::addr("EXTMEM", register), Word, value)
+                .unwrap();
+        }
+        assert_eq!(load(&mut b, vaddr), 0x4F02_06E8);
+        assert_eq!(b.ctx.diag.drain(), Vec::new());
+    }
+
+    #[test]
+    fn one_load_that_straddles_a_stale_page_is_one_occurrence_at_its_address() {
+        let mut b = bus_with_flash(SocConfig::default());
+        map(&mut b, 3, 1);
+        map(&mut b, 4, 2);
+        let vaddr = FLASH_DBUS_BASE + 0x3_FFFE; // two bytes of flash page 1, two of page 2
+        assert_eq!(load(&mut b, vaddr), 0xB1B0_A3A2);
+        program_byte(&mut b, 0x2_0000, 0x00);
+        assert_eq!(load(&mut b, vaddr), 0xB100_A3A2);
+        assert_eq!(b.ctx.diag.drain(), vec![stale(vaddr, 2)]);
+        assert_eq!(b.ctx.diag.occurrences(&stale(0, 2)), 1);
+        assert_eq!(b.ctx.diag.occurrences(&stale(0, 1)), 0);
+    }
+
+    #[test]
+    fn strict_mode_turns_a_window_read_that_may_differ_into_a_violation() {
+        let mut b = bus_with_flash(SocConfig {
+            strict_mmio: true,
+            ..SocConfig::default()
+        });
+        map(&mut b, 3, 1);
+        let vaddr = FLASH_DBUS_BASE + 0x3_0000;
+        load(&mut b, vaddr);
+        program_byte(&mut b, 0x1_0000, 0x00);
+        assert_eq!(
+            b.ctx.diag.take_violation(),
+            None,
+            "the write is no violation"
+        );
+        load(&mut b, vaddr);
+        assert_eq!(b.ctx.diag.take_violation(), Some(stale(vaddr, 1)));
+    }
+
     #[test]
     fn the_memories_keep_their_content_at_every_level_of_reset() {
         // TRM §6.1.1: a reset leaves the memories as they are. The RTC memory is the case the
````

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -717,6 +736,11 @@ mod tests {
             unsupported(PROTECTED),
             mapping(3, MappingFault::InvalidEntry),
             mapping(3, MappingFault::BeyondFlash),
+            Diagnostic::StaleFlashWindow {
+                vaddr: 0x3C00_E000,
+                page: 0,
+                pc: 0x4038_0000,
+            },
         ] {
             let mut strict = Diagnostics::new(true);
             strict.record(diagnostic.clone());
@@ -1084,4 +1108,35 @@ mod tests {
         );
         assert_eq!(d.occurrences(&listed[0]), 2, "the second one is counted");
     }
+
+    #[test]
+    fn a_window_read_that_may_differ_is_listed_once_per_flash_page_and_says_what_to_do() {
+        let stale = |vaddr, page| Diagnostic::StaleFlashWindow {
+            vaddr,
+            page,
+            pc: 0x4038_0000,
+        };
+        let mut d = Diagnostics::new(false);
+        d.record(stale(0x3C00_E000, 0));
+        d.record(stale(0x3C00_E004, 0));
+        d.record(stale(0x3C01_0000, 1));
+        assert_eq!(
+            d.drain(),
+            vec![stale(0x3C00_E000, 0), stale(0x3C01_0000, 1)],
+            "one line per flash page, with the first address read"
+        );
+        assert_eq!(d.occurrences(&stale(0, 0)), 2);
+        let message = stale(0x3C01_E000, 1).to_string();
+        for part in [
+            "0x3c01e000",
+            "pc 0x40380000",
+            "may differ from the device",
+            "flash 0x010000 to 0x01ffff",
+            "invalidate the cache",
+        ] {
+            assert!(message.contains(part), "{part:?} in {message:?}");
+        }
+        // The page is a number the firmware chose: the message of any of them can be formed.
+        assert!(stale(0, u32::MAX).to_string().contains("0xffffffffffff"));
+    }
 }
````

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -1132,7 +1132,17 @@ mod tests {
         const A0: u8 = 10;
         let mut m = calls_flash_code_around(&flash_program(0, &addi(A0, 0, 1).to_le_bytes()));
         m.run_until(Cycles(600));
-        assert_eq!(m.drain_diagnostics(), Vec::new());
+        // The program changed code the CPU had fetched, and nothing invalidated the cache: a
+        // device may run the function of before once more. The fetch is reported, at its address.
+        let window = crate::memmap::FLASH_IBUS_BASE;
+        assert_eq!(
+            m.drain_diagnostics(),
+            vec![Diagnostic::StaleFlashWindow {
+                vaddr: window,
+                page: 0,
+                pc: window,
+            }]
+        );
         assert_eq!(
             (
                 bus_word(&mut m, SRAM_DBUS_BASE),
````

The help of the command, which lists what `--strict-mmio` stops on:

````diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -497,7 +497,8 @@ fn run_help_documents_the_flash_option_and_what_strict_stops_on() {
     inkemu().args(["run", "--help"]).assert().code(0).stdout(
         contains("--flash")
             .and(contains("erased"))
-            .and(contains("flash that is not mapped")),
+            .and(contains("flash that is not mapped"))
+            .and(contains("window that may differ from the device")),
     );
 }
 
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib > /tmp/t6-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t6-red.log
cargo test -p emu-cli --test run_cli run_help > /tmp/t6-red2.log 2>&1; echo "exit=$?"; grep "FAILED$" /tmp/t6-red2.log
```

Expected: `exit=101`; the crate does not compile: ``cannot find type `MmuTable` in this scope`` (the tests of `extmem.rs`), ``no variant named `StaleFlashWindow` found for enum `diag::Diagnostic` ``, ``no method named `window_read` found``. Then `exit=101` and `test run_help_documents_the_flash_option_and_what_strict_stops_on ... FAILED`.

- [ ] **Step 3: The two events, and the flash page of an MMU entry**

````diff
--- a/crates/soc-esp32c3/src/ctx.rs
+++ b/crates/soc-esp32c3/src/ctx.rs
@@ -91,6 +91,13 @@ pub enum Event {
     MmuFault { vaddr: u32, content: u32 },
     /// To `EXTMEM`: a store reached a flash window.
     FlashWindowStore { instruction_bus: bool },
+    /// To `EXTMEM`, from SPI1: a program or an erase changed bytes of the 64 KiB flash pages
+    /// `first_page` to `last_page`.
+    FlashWritten { first_page: u32, last_page: u32 },
+    /// To `EXTMEM`, from its own `ICACHE_SYNC_CTRL`: software invalidated the cache range its
+    /// `ICACHE_SYNC_ADDR` and `ICACHE_SYNC_SIZE` name. It comes back as an event because what
+    /// the range holds is told by the MMU table, another device.
+    CacheInvalidated,
     /// Wake-up of the SYSTIMER: one of its comparators may have an alarm due.
     Systimer,
     /// Wake-up of UART 0 or 1: its transmitter may have gone idle.
@@ -348,9 +355,11 @@ impl Ctx {
     ///
     /// The rule for a device author: a signal is delivered before the instruction that posted it
     /// ends, and the delivery of signals is not bounded by a pass. So a device must not post a
-    /// signal from the delivery of a signal or of a wake-up (today only a write to `SYSTEM`
-    /// posts one, and no delivery writes `SYSTEM`): a chain of signals each posting the next
-    /// would never let the instruction end.
+    /// signal from the delivery of a signal or of a wake-up: a chain of signals each posting the
+    /// next would never let the instruction end. Today three register writes post one, and no
+    /// delivery writes any of the three registers: `SYSTEM.PERIP_RST_EN0/1` (module resets),
+    /// `SPI1.CMD` (a program or an erase changed the flash) and `EXTMEM.ICACHE_SYNC_CTRL` (a
+    /// cache invalidation).
     pub fn signal(&mut self, event: Event) {
         self.signals.push_back(event);
     }
````

````diff
--- a/crates/soc-esp32c3/src/periph/mmu.rs
+++ b/crates/soc-esp32c3/src/periph/mmu.rs
@@ -49,6 +49,15 @@ impl MmuTable {
         self.entries.get(index).copied()
     }
 
+    /// The 64 KiB flash page entry `index` maps; `None` when the entry is invalid or `index` lies
+    /// past the table.
+    #[must_use]
+    pub fn flash_page(&self, index: usize) -> Option<u32> {
+        self.entry(index)
+            .filter(|content| content & MMU_ENTRY_INVALID == 0)
+            .map(|content| content & PAGE_NUMBER_MASK)
+    }
+
     /// The flash offset `window_offset` (an address minus the base of its window) maps to.
     ///
     /// # Errors
````

- [ ] **Step 4: The cache state**

````diff
--- a/crates/soc-esp32c3/src/periph/extmem.rs
+++ b/crates/soc-esp32c3/src/periph/extmem.rs
@@ -4,8 +4,17 @@
 //! No cache line is modelled: the bus reads the flash through the MMU table at every access
 //! (decision D4). The operations software waits for (invalidate, preload, autoload, freeze)
 //! therefore complete at once, and the registers below only keep what software reads back.
+//!
+//! One thing a cache does is still followed, because it is where reading the flash live can differ
+//! from a device: a line filled from the flash keeps its bytes when SPI1 programs or erases them,
+//! until software invalidates it (CM §3, "Stale data"). [`Extmem::window_read`],
+//! [`Extmem::flash_written`] and [`Extmem::invalidate`] keep, per 64 KiB flash page, whether a
+//! window read may now return other bytes than a device would. The bus reports such a read; it
+//! still returns the bytes of the flash.
+use super::mmu::{MMU_ENTRIES, MMU_PAGE_BYTES, MmuTable};
 use super::{Accepted, EXTMEM_BASE, Storage};
-use crate::ctx::{Ctx, Source};
+use crate::ctx::{Ctx, Event, Source};
+use crate::memmap::{FLASH_DBUS_BASE, FLASH_IBUS_BASE, FLASH_WINDOW_SIZE};
 use emu_core::Peripheral;
 use svd_regs::esp32c3::addr;
 
@@ -16,6 +25,8 @@ const fn reg(name: &str) -> u32 {
 const ICACHE_CTRL: u32 = reg("ICACHE_CTRL");
 const ICACHE_CTRL1: u32 = reg("ICACHE_CTRL1");
 const ICACHE_SYNC_CTRL: u32 = reg("ICACHE_SYNC_CTRL");
+const ICACHE_SYNC_ADDR: u32 = reg("ICACHE_SYNC_ADDR");
+const ICACHE_SYNC_SIZE: u32 = reg("ICACHE_SYNC_SIZE");
 const ICACHE_PRELOAD_CTRL: u32 = reg("ICACHE_PRELOAD_CTRL");
 const ICACHE_AUTOLOAD_CTRL: u32 = reg("ICACHE_AUTOLOAD_CTRL");
 const ICACHE_FREEZE: u32 = reg("ICACHE_FREEZE");
@@ -33,6 +44,52 @@ const CACHE_MMU_FAULT_VADDR: u32 = reg("CACHE_MMU_FAULT_VADDR");
 /// SVD gives reset 1 (`INVALIDATE_ENA` set, `DONE` clear), which reads as a tag initialisation that
 /// ends before the ROM runs; the ROM sets bit 0 and waits for bit 1 either way (CM §9 item 5).
 const SYNC_DONE: u32 = 1 << 1;
+/// `ICACHE_SYNC_CTRL` bit 0, `INVALIDATE_ENA`: starts the invalidation of the range in
+/// `ICACHE_SYNC_ADDR` and `ICACHE_SYNC_SIZE` (`ROM3:Cache_Invalidate_ICache_Items@0x40050f28`:
+/// `SYNC_ADDR = addr`, `SYNC_SIZE[22:0] = n`, `SYNC_CTRL |= 1`; CM §4.1).
+const SYNC_INVALIDATE: u32 = 1 << 0;
+/// `ICACHE_SYNC_SIZE` [22:0]: the number of cache lines, not of bytes. `Cache_Invalidate_Addr`
+/// called for `0x10000` bytes writes `0x800` (CM §4.1, measured).
+const SYNC_LINES_MASK: u32 = 0x7F_FFFF;
+/// A cache line (`ROM3:Cache_Get_ICache_Line_Size@0x40050e4c` returns 32; CM §3).
+const LINE_BYTES: u64 = 32;
+/// The 64 KiB pages of the largest flash an MMU entry can name: its page number has 8 bits.
+const FLASH_PAGES: u32 = 256;
+const PAGE_SET_WORDS: usize = 4;
+const WORD_BITS: u32 = u64::BITS;
+
+/// A set of 64 KiB flash pages, one bit each: constant size whatever the firmware does.
+#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
+struct PageSet([u64; PAGE_SET_WORDS]);
+
+impl PageSet {
+    /// The word that holds `page` and the bit of the page in it; `None` past the 256 pages an
+    /// MMU entry can name, so that a page number out of range is in no set and changes none.
+    fn slot(&mut self, page: u32) -> Option<(&mut u64, u64)> {
+        let word = self.0.get_mut(usize::try_from(page / WORD_BITS).ok()?)?;
+        Some((word, 1 << (page % WORD_BITS)))
+    }
+
+    fn contains(&self, page: u32) -> bool {
+        usize::try_from(page / WORD_BITS)
+            .ok()
+            .and_then(|word| self.0.get(word))
+            .is_some_and(|word| word & (1 << (page % WORD_BITS)) != 0)
+    }
+
+    fn insert(&mut self, page: u32) {
+        if let Some((word, bit)) = self.slot(page) {
+            *word |= bit;
+        }
+    }
+
+    fn remove(&mut self, page: u32) {
+        if let Some((word, bit)) = self.slot(page) {
+            *word &= !bit;
+        }
+    }
+}
+
 /// `ICACHE_PRELOAD_CTRL`: bit 1 `DONE` (always set: nothing is ever preloading), bit 2 `ORDER` (stored).
 const PRELOAD_DONE: u32 = 1 << 1;
 const PRELOAD_ORDER: u32 = 1 << 2;
@@ -83,6 +140,7 @@ const ILLEGAL_GATES: [(u32, u32); 5] = [
 ];
 
 const WHY_OPERATION: &str = "cache operation: completes at once, there are no cache lines";
+const WHY_INVALIDATE: &str = "cache invalidation: completes at once, there are no cache lines; it only ends the stale-read warning of the flash pages it covers";
 const ACCEPTED: [Accepted; 12] = [
     Accepted::svd(
         "EXTMEM",
@@ -97,9 +155,9 @@ const ACCEPTED: [Accepted; 12] = [
         "SP1b-4",
     ),
     Accepted::svd("EXTMEM", "ICACHE_TAG_POWER_CTRL", "tag memory power", "SP8"),
-    Accepted::svd("EXTMEM", "ICACHE_SYNC_CTRL", WHY_OPERATION, "SP1b-4"),
-    Accepted::svd("EXTMEM", "ICACHE_SYNC_ADDR", WHY_OPERATION, "SP1b-4"),
-    Accepted::svd("EXTMEM", "ICACHE_SYNC_SIZE", WHY_OPERATION, "SP1b-4"),
+    Accepted::svd("EXTMEM", "ICACHE_SYNC_CTRL", WHY_INVALIDATE, "SP1b-4"),
+    Accepted::svd("EXTMEM", "ICACHE_SYNC_ADDR", WHY_INVALIDATE, "SP1b-4"),
+    Accepted::svd("EXTMEM", "ICACHE_SYNC_SIZE", WHY_INVALIDATE, "SP1b-4"),
     Accepted::svd("EXTMEM", "ICACHE_PRELOAD_CTRL", WHY_OPERATION, "SP1b-4"),
     Accepted::svd("EXTMEM", "ICACHE_AUTOLOAD_CTRL", WHY_OPERATION, "SP1b-4"),
     Accepted::svd(
@@ -130,12 +188,20 @@ pub struct Extmem {
     fault_content: u32,
     fault_vaddr: u32,
     generation: u64,
+    /// The flash pages read through a window since the cache was last invalidated for them: the
+    /// pages a device may hold lines of.
+    cached: PageSet,
+    /// Those of `cached` a program or an erase has changed since: a device may still answer a
+    /// window read of them with the bytes of before.
+    stale: PageSet,
 }
 
 impl Extmem {
     #[must_use]
     pub fn new() -> Extmem {
         Extmem {
+            cached: PageSet::default(),
+            stale: PageSet::default(),
             registers: Storage::accepted(EXTMEM_BASE, &ACCEPTED),
             preload_order: 0,
             autoload: 0,
@@ -156,6 +222,82 @@ impl Extmem {
         self.generation
     }
 
+    /// A read through a flash window reached the 64 KiB flash page `page`. Returns whether a
+    /// device may answer with other bytes than the flash now holds: the page was read through a
+    /// window, then programmed or erased, and the cache was not invalidated for it since.
+    ///
+    /// Constant cost: one bit set and one bit tested. A page number past the 256 an MMU entry can
+    /// name is in no set.
+    pub(crate) fn window_read(&mut self, page: u32) -> bool {
+        self.cached.insert(page);
+        self.stale.contains(page)
+    }
+
+    /// SPI1 changed bytes of the flash pages `first_page` to `last_page`. The pages a window read
+    /// since their last invalidation become stale; a page no window read is in no cache line.
+    ///
+    /// The two numbers come from an address the firmware wrote: the loop is bounded by
+    /// [`FLASH_PAGES`], whatever they are.
+    pub(crate) fn flash_written(&mut self, first_page: u32, last_page: u32) {
+        for page in first_page..=last_page.min(FLASH_PAGES - 1) {
+            if self.cached.contains(page) {
+                self.stale.insert(page);
+            }
+        }
+    }
+
+    /// Software invalidated the cache lines of the range in `ICACHE_SYNC_ADDR` and
+    /// `ICACHE_SYNC_SIZE`: the flash pages the range covers are in no line any more.
+    ///
+    /// - Address 0 with size 0 is every line (`ROM3:Cache_Invalidate_ICache_All@0x40051126` is
+    ///   `Cache_Invalidate_ICache_Items(0, 0)`; `Cache_Disable_ICache` and
+    ///   `Cache_Set_Default_Mode` call it; CM §4.1).
+    /// - Otherwise the range is `size` lines of 32 bytes from an address of one of the two
+    ///   windows. Both windows go through the one MMU table and the one cache (CM §1, §3), so the
+    ///   range names MMU entries whichever window it is given in. A page is dropped when a whole
+    ///   entry that maps it lies in the range, as `spi_flash_check_and_flush_cache` asks
+    ///   (`IDF:components/spi_flash/flash_mmap.c:306-321`: `cache_hal_invalidate_addr(vaddr,
+    ///   SPI_FLASH_MMU_PAGE_SIZE)`). A range that covers part of an entry drops nothing: the model
+    ///   keeps one bit per page, and the lines left in the cache would still be stale.
+    ///
+    /// ASSUMPTION: a size of 0 at another address than 0, and an address outside the two windows,
+    /// invalidate nothing. No ROM or ESP-IDF function issues either (`Cache_Invalidate_Addr`
+    /// returns 1 before it writes a register when the range is not inside one window, CM §4.1).
+    /// Reading a window, programming it, issuing such a request and reading again on a device
+    /// settles both.
+    ///
+    /// The loop is bounded by the 128 entries of the table, whatever the two registers hold.
+    pub(crate) fn invalidate(&mut self, mmu: &MmuTable) {
+        let addr = self.registers.read(ICACHE_SYNC_ADDR).unwrap_or(0);
+        let lines = self.registers.read(ICACHE_SYNC_SIZE).unwrap_or(0) & SYNC_LINES_MASK;
+        if lines == 0 {
+            if addr == 0 {
+                self.cached = PageSet::default();
+                self.stale = PageSet::default();
+            }
+            return;
+        }
+        let Some(start) = [FLASH_DBUS_BASE, FLASH_IBUS_BASE]
+            .into_iter()
+            .filter_map(|base| addr.checked_sub(base))
+            .find(|offset| *offset < FLASH_WINDOW_SIZE)
+        else {
+            return;
+        };
+        let page_bytes = u64::from(MMU_PAGE_BYTES);
+        let end = u64::from(start) + u64::from(lines) * LINE_BYTES;
+        let first = u64::from(start).div_ceil(page_bytes);
+        for entry in (first..end / page_bytes).take(MMU_ENTRIES) {
+            let page = usize::try_from(entry)
+                .ok()
+                .and_then(|index| mmu.flash_page(index));
+            if let Some(page) = page {
+                self.cached.remove(page);
+                self.stale.remove(page);
+            }
+        }
+    }
+
     /// Records an access through an invalid MMU entry (`CACHE_ILG_INT_ST`, `CACHE_MMU_FAULT_*`; CM §6.1).
     pub(crate) fn latch_mmu_fault(&mut self, ctx: &mut Ctx, vaddr: u32, content: u32) {
         self.illegal_status |= MMU_ENTRY_FAULT;
@@ -226,8 +368,15 @@ impl Peripheral<Ctx> for Extmem {
 
     fn write(&mut self, ctx: &mut Ctx, offset: u32, value: u32) -> bool {
         match offset {
-            // The invalidation has nothing to invalidate; `SYNC_DONE` already reads set.
-            ICACHE_SYNC_CTRL => {}
+            // There is no line to invalidate and `SYNC_DONE` already reads set. What the request
+            // ends is the stale state of the pages it covers; which pages those are is told by
+            // the MMU table, another device, so the request comes back as an event
+            // (`Extmem::invalidate`).
+            ICACHE_SYNC_CTRL => {
+                if value & SYNC_INVALIDATE != 0 {
+                    ctx.signal(Event::CacheInvalidated);
+                }
+            }
             ICACHE_PRELOAD_CTRL => self.preload_order = value & PRELOAD_ORDER,
             ICACHE_AUTOLOAD_CTRL => self.autoload = value & AUTOLOAD_CONFIG,
             ICACHE_FREEZE => self.freeze = value & (FREEZE_ENA | FREEZE_MODE),
````

- [ ] **Step 5: SPI1 tells the cache which pages changed; the events are delivered**

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -1,8 +1,9 @@
 //! SPI0 and SPI1, the two controllers that share the flash chip (`spi_mem_reg.h`; reports BC §D.1,
 //! FB §B and §D). SPI1 runs the commands software asks for; SPI0 refills the cache, which this
 //! emulator does not model (the bus reads the flash directly), so SPI0 is configuration storage.
+use super::mmu::MMU_PAGE_BYTES;
 use super::{Accepted, SPI0_BASE, SPI1_BASE, Storage};
-use crate::ctx::Ctx;
+use crate::ctx::{Ctx, Event};
 use crate::diag::Unsupported;
 use crate::flash::{FlashAddress, FlashCommand, FlashDevice, FlashJournal, FlashWrite};
 use emu_core::Peripheral;
@@ -216,6 +217,23 @@ impl Peripheral<Ctx> for Spi0 {
     }
 }
 
+/// The first and the last 64 KiB flash page `write` holds bytes of, the unit of the MMU and of
+/// what the cache is told; `None` for a status write, which changes no content.
+fn cache_pages(write: &FlashWrite) -> Option<(u32, u32)> {
+    match *write {
+        // A program stays inside its 256-byte page, so inside one 64 KiB page.
+        FlashWrite::Program { address, .. } => {
+            let page = address / MMU_PAGE_BYTES;
+            Some((page, page))
+        }
+        FlashWrite::Erase { start, len } => {
+            let last = start.saturating_add(len.saturating_sub(1));
+            Some((start / MMU_PAGE_BYTES, last / MMU_PAGE_BYTES))
+        }
+        FlashWrite::Status { .. } => None,
+    }
+}
+
 #[derive(Debug)]
 pub struct Spi1 {
     registers: Storage,
@@ -280,9 +298,20 @@ impl Spi1 {
         data_in_len: usize,
     ) -> Option<Vec<u8>> {
         let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
+        let before = self.flash.generation();
         match self.flash.execute(command, &mut data_in) {
             Ok(written) => {
                 if let Some(write) = written {
+                    // The cache is told of the bytes that changed, not of a command that left
+                    // the content as it was: only the former can make a cached line stale.
+                    if self.flash.generation() != before
+                        && let Some((first_page, last_page)) = cache_pages(&write)
+                    {
+                        ctx.signal(Event::FlashWritten {
+                            first_page,
+                            last_page,
+                        });
+                    }
                     self.journal.record(write);
                 }
                 Some(data_in)
````

````diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -473,6 +473,11 @@ impl Peripherals {
             Event::FlashWindowStore { instruction_bus } => {
                 self.extmem.latch_store(ctx, instruction_bus);
             }
+            Event::FlashWritten {
+                first_page,
+                last_page,
+            } => self.extmem.flash_written(first_page, last_page),
+            Event::CacheInvalidated => self.extmem.invalidate(&self.mmu),
             Event::Systimer => self.systimer.wake(ctx),
             Event::Timg(Group::Timg0) => self.timg0.wake(ctx),
             Event::Timg(Group::Timg1) => self.timg1.wake(ctx),
````

- [ ] **Step 6: The diagnostic, and the bus that records it**

````diff
--- a/crates/soc-esp32c3/src/diag.rs
+++ b/crates/soc-esp32c3/src/diag.rs
@@ -1,6 +1,7 @@
 //! What the emulator has to tell the user about the run: accesses it does not model, CPU
 //! exceptions, resets.
 use crate::flash::FlashRefusal;
+use crate::periph::mmu::MMU_PAGE_BYTES;
 use crate::reset::ResetCause;
 use cpu_rv32::Exception;
 use std::collections::BTreeMap;
@@ -219,6 +220,11 @@ pub enum Diagnostic {
         fault: MappingFault,
         pc: u32,
     },
+    /// A read through a flash window that a device may answer with other bytes. The emulator
+    /// reads the flash at every access; a device reads its cache, which keeps the bytes a 64 KiB
+    /// flash `page` held before SPI1 programmed or erased it, until software invalidates it
+    /// (report CM §3). The read returned the bytes the flash holds now.
+    StaleFlashWindow { vaddr: u32, page: u32, pc: u32 },
     /// A synchronous exception the CPU took. It is no error of the emulator: the firmware's trap
     /// handler runs, as on the device.
     Exception {
@@ -292,6 +298,8 @@ impl Diagnostic {
                 Unsupported::FlashSuspend { .. } => (17, 0, 0),
             },
             Diagnostic::FlashMapping { entry, fault, .. } => (10, entry, fault as u32),
+            // One line per flash page: the firmware reads the page again at many addresses.
+            Diagnostic::StaleFlashWindow { page, .. } => (21, page, 0),
             Diagnostic::Reset { cause, pc, .. } => (14, pc, cause.code()),
         }
     }
@@ -305,6 +313,7 @@ impl Diagnostic {
                 | Diagnostic::SubWordWrite { .. }
                 | Diagnostic::Unsupported { .. }
                 | Diagnostic::FlashMapping { .. }
+                | Diagnostic::StaleFlashWindow { .. }
         )
     }
 }
@@ -365,6 +374,15 @@ impl fmt::Display for Diagnostic {
                     "access to the flash window at {vaddr:#010x} through MMU entry {entry} (content {content:#05x}, pc {pc:#010x}), which names a page beyond the end of the flash: 0 was returned; check the flash size of the device profile"
                 ),
             },
+            Diagnostic::StaleFlashWindow { vaddr, page, pc } => {
+                // 64 bits: `page` is a number the firmware chose through an MMU entry.
+                let start = u64::from(page) * u64::from(MMU_PAGE_BYTES);
+                let end = start + u64::from(MMU_PAGE_BYTES) - 1;
+                write!(
+                    f,
+                    "read of the flash window at {vaddr:#010x} (pc {pc:#010x}) may differ from the device: flash {start:#08x} to {end:#08x} was read through a window, then programmed or erased, and the cache was not invalidated for it since; the bytes the flash holds now were returned, where a device may still return those of before; invalidate the cache for that range after the write, as ESP-IDF's esp_flash and esp_partition functions do for mapped flash"
+                )
+            }
             Diagnostic::Exception {
                 cause,
                 tval,
@@ -441,7 +459,8 @@ impl Diagnostics {
     }
 
     /// In strict mode, the first violation; `None` otherwise or once taken. A violation is the first
-    /// unmodeled access, sub-word register write, unsupported operation or invalid flash mapping.
+    /// unmodeled access, sub-word register write, unsupported operation, invalid flash mapping or
+    /// window read that may differ from the device.
     pub fn take_violation(&mut self) -> Option<Diagnostic> {
         self.violation.take()
     }
````

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -211,16 +211,24 @@ impl SocBus {
         // at most 4 bytes, so it touches at most two entries, one after the other: remembering the
         // last fault is enough to report each (entry, fault) once.
         let mut reported: Option<UnbackedByte> = None;
+        // The flash page of a byte a device may answer otherwise (`Extmem::window_read`). One
+        // access is one event, as above: the first such page is the one reported.
+        let mut stale: Option<u32> = None;
         // Byte by byte: a misaligned access may straddle two 64 KiB pages, each with its own entry.
         for (index, byte) in (0u32..len).zip(bytes.iter_mut()) {
             // INVARIANT: `locate` checked that the whole access lies inside the window, so
             // `window_offset + index` stays below `FLASH_WINDOW_SIZE` and cannot overflow.
             match self.flash_offset(window_offset + index) {
-                Ok(offset) => self
-                    .periph
-                    .spi1
-                    .flash()
-                    .read(offset, std::slice::from_mut(byte)),
+                Ok(offset) => {
+                    self.periph
+                        .spi1
+                        .flash()
+                        .read(offset, std::slice::from_mut(byte));
+                    let page = offset / MMU_PAGE_BYTES;
+                    if self.periph.extmem.window_read(page) {
+                        stale.get_or_insert(page);
+                    }
+                }
                 Err(unbacked) if reported == Some(unbacked) => {}
                 Err(unbacked) => {
                     // The fault registers and the diagnostic name the address of the access.
@@ -239,6 +247,13 @@ impl SocBus {
                 }
             }
         }
+        if let Some(page) = stale {
+            self.ctx.diag.record(Diagnostic::StaleFlashWindow {
+                vaddr,
+                page,
+                pc: self.ctx.pc(),
+            });
+        }
         ReadOutcome {
             value: u32::from_le_bytes(bytes) & size_mask(size),
             stall: 0,
@@ -294,10 +309,12 @@ impl SocBus {
 /// Delivers the events devices posted with [`Ctx::signal`], oldest first.
 ///
 /// The loop ends: the only code that posts a signal is a write to `SYSTEM.PERIP_RST_EN0/1`
-/// (`System::write`), which a register access reaches and a delivery does not. A delivery resets a
-/// module, and no module reset writes a register of `SYSTEM` or posts a signal. So the signals
-/// there are to deliver are those one register access posted (at most one per module), and
-/// delivering them posts none.
+/// (`System::write`: module resets), to `SPI1.CMD` (`Spi1::transfer`: a program or an erase
+/// changed the flash) or to `EXTMEM.ICACHE_SYNC_CTRL` (`Extmem::write`: a cache invalidation),
+/// which a register access reaches and a delivery does not. A delivery resets a module, or marks
+/// or clears flash pages in the cache state: none writes one of those registers or posts a
+/// signal. So the signals there are to deliver are those one register access posted (at most one
+/// per module, or one in all for the two others), and delivering them posts none.
 fn settle_signals(periph: &mut Peripherals, ctx: &mut Ctx) {
     while let Some(event) = ctx.take_signal() {
         periph.deliver(ctx, event);
````

````diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -246,6 +246,7 @@ const fn journal_prefix(diagnostic: &Diagnostic) -> &'static str {
         | Diagnostic::SubWordWrite { .. }
         | Diagnostic::Unsupported { .. }
         | Diagnostic::FlashMapping { .. }
+        | Diagnostic::StaleFlashWindow { .. }
         | Diagnostic::Exception { .. }
         | Diagnostic::Suppressed => "warning",
     }
````

The help says what the option now stops on:

````diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -79,8 +79,9 @@ enum Command {
         )]
         max_time: Option<u64>,
         /// Stop at the first access to a register the emulator does not model, 8/16-bit write to a
-        /// peripheral register, operation the emulator cannot perform faithfully, or access to flash
-        /// that is not mapped (exit code 3).
+        /// peripheral register, operation the emulator cannot perform faithfully, access to flash
+        /// that is not mapped, or read through a flash window that may differ from the device
+        /// (exit code 3).
         #[arg(long)]
         strict_mmio: bool,
         /// Which console of the device goes to standard output.
````

````diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -22,7 +22,8 @@ Exit codes:
      pressed, or the reader of standard output went away
   2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
   3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
-     emulator cannot perform faithfully, or flash that is not mapped)
+     emulator cannot perform faithfully, flash that is not mapped, or a read through a flash
+     window that may differ from the device)
   4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
      has anything scheduled
 130  Ctrl-C was pressed a second time before the run had stopped: no summary was printed";
````

- [ ] **Step 7: Run the suite, regenerate the table**

```bash
INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t6-bless.log 2>&1; echo "exit=$?"
cargo test --workspace > /tmp/t6.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t6.log
```

Expected: `exit=0` twice and `894`. The table:

````diff
--- a/docs/registers-accepted-without-effect.md
+++ b/docs/registers-accepted-without-effect.md
@@ -130,9 +130,9 @@ its first section says which plan takes what.
 | `0x600c4000` | `EXTMEM.ICACHE_CTRL` | 1 | cache enable: the flash windows are always readable (D5) | SP1b-4 |
 | `0x600c4004` | `EXTMEM.ICACHE_CTRL1` | 1 | bus gating of the flash windows (SHUT_IBUS, SHUT_DBUS; D5) | SP1b-4 |
 | `0x600c4008` | `EXTMEM.ICACHE_TAG_POWER_CTRL` | 1 | tag memory power | SP8 |
-| `0x600c4028` | `EXTMEM.ICACHE_SYNC_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
-| `0x600c402c` | `EXTMEM.ICACHE_SYNC_ADDR` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
-| `0x600c4030` | `EXTMEM.ICACHE_SYNC_SIZE` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
+| `0x600c4028` | `EXTMEM.ICACHE_SYNC_CTRL` | 1 | cache invalidation: completes at once, there are no cache lines; it only ends the stale-read warning of the flash pages it covers | SP1b-4 |
+| `0x600c402c` | `EXTMEM.ICACHE_SYNC_ADDR` | 1 | cache invalidation: completes at once, there are no cache lines; it only ends the stale-read warning of the flash pages it covers | SP1b-4 |
+| `0x600c4030` | `EXTMEM.ICACHE_SYNC_SIZE` | 1 | cache invalidation: completes at once, there are no cache lines; it only ends the stale-read warning of the flash pages it covers | SP1b-4 |
 | `0x600c4034` | `EXTMEM.ICACHE_PRELOAD_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
 | `0x600c4040` | `EXTMEM.ICACHE_AUTOLOAD_CTRL` | 1 | cache operation: completes at once, there are no cache lines | SP1b-4 |
 | `0x600c40a8` | `EXTMEM.CACHE_WRAP_AROUND_CTRL` | 1 | wrap-around read mode | SP1b-4 |
````

Ignored tests: the ROM, the bootloader and ESP-IDF must never raise the new diagnostic (the strict runs would stop on it).

```bash
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t6-i1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 10 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware > /tmp/t6-i2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 7 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t6-i3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 2 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored > /tmp/t6-i4.log 2>&1; echo "exit=$?"
target/release/inkemu run --device x4 --strict-mmio --max-cycles 40000000 --trace-hash --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" > /dev/null 2> /tmp/t6-hash.log; echo "exit=$?"; grep trace-hash /tmp/t6-hash.log
```

Expected: `exit=0` five times and `trace-hash 10174cc2726c3d77`.

- [ ] **Step 8: ADR 0002, revised**

The ADR said the flash is read-only and listed the stale line as a known gap. It is rewritten where that is no longer true (points 4 and 5), gains point 6 and the limits of a state kept per page, and its last consequence records the revision and when to reopen it.

````diff
--- a/docs/adr/0002-flash-model-and-cache-windows.md
+++ b/docs/adr/0002-flash-model-and-cache-windows.md
@@ -1,7 +1,7 @@
 # ADR 0002 — Modèle de la flash et fenêtres de cache
 
 - Date : 2026-10-11
-- Statut : accepté (plan SP1b-2)
+- Statut : accepté (plan SP1b-2), révisé le 2026-10-12 (plan SP1b-3b : la flash s'écrit ; points 2, 4, 5 et 6, conséquences)
 
 ## Contexte
 
@@ -12,15 +12,18 @@ Il faut décider ce que l'émulateur modélise de cet ensemble pour que la ROM e
 ## Décision
 
 1. **Les fenêtres lisent la flash en direct à travers la table MMU.** Aucune ligne de cache, aucune étiquette, aucun défaut de cache n'est modélisé. Un accès traduit son adresse avec l'entrée de sa page de 64 Kio et lit le contenu de la flash à cet instant.
-2. **Le contrôleur de cache se réduit à une logique d'achèvement et à du stockage.** Les opérations que le logiciel attend (invalidation, préchargement, chargement automatique, gel) se terminent aussitôt ; les bits d'activation et de fermeture des bus sont mémorisés et ne sont pas appliqués.
+2. **Le contrôleur de cache se réduit à une logique d'achèvement et à du stockage.** Les opérations que le logiciel attend (invalidation, préchargement, chargement automatique, gel) se terminent aussitôt ; les bits d'activation et de fermeture des bus sont mémorisés et ne sont pas appliqués. Une seule opération garde un effet, sur un diagnostic et non sur ce que le firmware lit : l'invalidation, qui met fin à l'état « périmé » du point 6.
 3. **Ce qui ne peut pas être juste en silence est signalé.** Un accès à travers une entrée invalide, ou vers une page située au-delà de la fin de la flash, renvoie 0, verrouille `CACHE_ILG_INT_ST` et `CACHE_MMU_FAULT_*`, et produit un diagnostic nommé ; `--strict-mmio` s'y arrête. Une écriture dans une fenêtre lève une faute d'accès en écriture.
-4. **La flash est pour l'instant en lecture seule et appartient au modèle de SPI1**, derrière le trait `FlashDevice`. Une image plus courte que la puce est acceptée : les octets au-delà de sa fin se lisent `0xFF`, comme une zone effacée ; une image plus longue que la puce est refusée. SPI1 implémente ses commandes câblées et le transfert utilisateur générique ; le modèle de la puce répond aux commandes que la chaîne de démarrage envoie, d'après les mesures, et refuse tout autre opcode en le nommant.
-5. **Le code décodé suit la table.** `Bus::page_version` des deux fenêtres change à chaque écriture dans la table MMU ou dans les bits d'activation du cache : le cache de décodage du CPU abandonne ainsi le code de flash qui a pu être déplacé.
+4. **La flash appartient au modèle de SPI1, derrière le trait `FlashDevice`, et elle s'écrit.** Une image plus courte que la puce est acceptée : les octets au-delà de sa fin se lisent `0xFF`, comme une zone effacée ; une image plus longue que la puce est refusée. SPI1 implémente ses commandes câblées, avec leur adresse et leurs données, et le transfert utilisateur générique ; une écriture de `CMD` exécute une commande et une seule. La puce programme (chaque octet ne peut qu'effacer des bits, l'adresse se replie dans sa page de 256 octets), efface un secteur de 4 Kio, un bloc de 64 Kio ou la puce entière, et reçoit son registre d'état ; elle ignore, comme la pièce, une écriture que le verrou d'écriture ne précède pas, et le verrou retombe quand l'opération s'achève. Chaque opération s'achève aussitôt : le bit d'occupation ne se lève jamais, les durées réelles reviennent au sous-projet 8. Les lectures de données passent par six commandes. Tout le reste est refusé en le nommant : un opcode inconnu, une commande connue dans une trame qui ne l'est pas, une adresse au-delà de la fin de la puce (la pièce se replierait, ou ignorerait les bits qu'elle n'a pas : le modèle ne devine pas), un programme ou un effacement quand un bit de protection de bloc est levé, une demande de suspension.
+5. **Le code décodé suit la table et le contenu.** `Bus::page_version` des deux fenêtres change à chaque écriture dans la table MMU ou dans les bits d'activation du cache, et chaque fois qu'un programme ou un effacement change un octet de la flash : le cache de décodage du CPU abandonne ainsi le code de flash qui a pu être déplacé ou réécrit.
+6. **Une lecture de fenêtre qui peut différer du silicium est signalée.** Le point 1 reste : la fenêtre montre la flash telle qu'elle est. Sur le silicium, une ligne de cache garde les octets qu'elle a chargés jusqu'à ce que le logiciel l'invalide (`docs/research/esp32c3-cache-mmu.md`, section 3). Le modèle suit donc, par page de flash de 64 Kio et dans deux ensembles de 256 bits, les pages lues à travers une fenêtre depuis leur dernière invalidation, et parmi elles celles qu'un programme ou un effacement a modifiées depuis. Une lecture de fenêtre dans une page de ce second ensemble renvoie le contenu actuel de la flash et produit un diagnostic nommé, qui dit « peut différer » ; `--strict-mmio` s'y arrête. Une invalidation retire des deux ensembles les pages des entrées MMU qu'elle couvre en entier (`Cache_Invalidate_ICache_Items`), ou toutes les pages quand elle porte sur tout le cache (`Cache_Invalidate_ICache_All`, que `Cache_Disable_ICache` appelle). Couper puis rétablir le cache (`Cache_Suspend_ICache`, `Cache_Resume_ICache`) n'invalide rien, comme sur la puce.
 
 ## Conséquences
 
 - **ROM et bootloader non modifiés** : la ROM masquée et le bootloader ESP-IDF s'exécutent tels quels. Chacune de leurs attentes se termine, et les quelque 1 300 écritures dans la table que fait le bootloader à chaque démarrage (mesure sur QEMU, `docs/research/esp32c3-bootloader-accesses.md`, §2, ligne 31) ne demandent aucun suivi d'invalidation.
 - **Temps** : un défaut du cache de flash ne coûte aucun temps. Le modèle de temps reste « non calibré » ; le coût d'un défaut (spec, section 4.2) revient au plan SP1b-4, par `Bus::fetch_stall`, et sa calibration au sous-projet 8.
-- **Écart connu avec le silicium** : un firmware qui réécrit la flash puis relit une zone projetée **sans** invalider le cache verra ici les nouvelles données, et les anciennes sur le silicium. Cela ne compte qu'une fois les écritures en flash disponibles (SP1b-3b) ; SP1b-3b devra choisir entre cette vue en direct et des lignes qui restent périmées jusqu'à leur invalidation.
-- **Hypothèse** : le comportement d'une entrée invalide est une hypothèse (`ASSUMPTION:` dans `bus.rs`). Le lockstep de SP1b-4, puis un appareil réel, la trancheront. Un firmware sain ne l'atteint jamais : le test du jalon tourne en mode strict.
-- **Révision de l'ADR** : à rouvrir quand les écritures en flash arriveront, ou si une mesure sur le matériel contredit le point 3.
+- **Écart connu avec le silicium, désormais nommé** : un firmware qui réécrit la flash puis relit une zone projetée **sans** invalider le cache voit ici les nouvelles données, et peut voir les anciennes sur le silicium. Le plan SP1b-3b a gardé la vue en direct et signale cette lecture (point 6) plutôt que de modéliser des lignes périmées. La ROM, le bootloader et ESP-IDF ne déclenchent jamais ce diagnostic : ils invalident ce qu'ils ont écrit (`spi_flash_check_and_flush_cache`) ou tout le cache avant de reprojeter (`bootloader_mmap`), ce que vérifient les tests du démarrage sur la disposition OTA et de la vie du firmware NVS, en mode strict.
+- **Limites du diagnostic** : il se prononce par page de 64 Kio et non par ligne de 32 octets. Il se tait dans un cas où le silicium pourrait renvoyer d'anciens octets : une page de flash projetée à deux adresses, lue aux deux, et invalidée à une seule. Il parle dans trois cas où le silicium relirait la flash : une invalidation qui ne couvre qu'une partie d'une entrée ne retire rien, même si elle couvre justement les lignes modifiées ; une ligne évincée par le remplissage du cache, qui n'a que 16 Kio, n'est pas suivie ; une entrée reprojetée vers une autre page avant d'être invalidée laisse l'ancienne page marquée. C'est pourquoi le diagnostic dit « peut différer », et non « diffère ». Un cache fidèle à la ligne revient au plan SP1b-4, avec le lockstep qui le rend vérifiable.
+- **Une autre lecture reste en direct sans diagnostic** : celle d'une adresse dont l'entrée MMU vient de changer sans invalidation. Le silicium peut y renvoyer la ligne de l'ancienne page ; ESP-IDF invalide après chaque projection. Elle revient au même cache fidèle à la ligne (SP1b-4).
+- **Hypothèses** : le comportement d'une entrée invalide est une hypothèse (`ASSUMPTION:` dans `bus.rs`), ainsi que l'effet d'une invalidation de taille nulle à une adresse non nulle ou hors des deux fenêtres (`ASSUMPTION:` dans `extmem.rs` : elle ne retire rien). Le lockstep de SP1b-4, puis un appareil réel, les trancheront. Un firmware sain n'atteint ni l'une ni l'autre : les tests du jalon tournent en mode strict.
+- **Révision de l'ADR** : révisé le 2026-10-12 quand les écritures en flash sont arrivées. À rouvrir quand SP1b-4 modélisera les lignes du cache, ou si une mesure sur le matériel contredit le point 3 ou le point 6.
````

- [ ] **Step 9: Mutations**

Each row: apply the change, run `cargo test -p soc-esp32c3 --lib`, see the test fail, undo. `E` is `periph/extmem.rs`.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `E`: remove `ctx.signal(Event::CacheInvalidated);` | `periph::extmem::tests::only_the_invalidate_bit_of_the_sync_register_asks_for_an_invalidation`, `bus::tests::an_invalidation_of_the_page_ends_the_report_until_it_is_written_again` |
| 2 | `E`, `flash_written`: remove the condition `if self.cached.contains(page) {` around `self.stale.insert(page);` | `periph::extmem::tests::a_page_written_before_any_window_read_it_is_not_stale`, `bus::tests::flash_written_before_a_window_read_it_or_left_as_it_was_is_not_reported` |
| 3 | `E`, `window_read`: remove `self.cached.insert(page);` | `periph::extmem::tests::a_page_read_through_a_window_then_written_is_stale_until_the_cache_is_invalidated` |
| 4 | `E`, `invalidate`: remove `self.cached.remove(page);` | `periph::extmem::tests::a_page_written_before_any_window_read_it_is_not_stale` |
| 5 | `E`: `u64::from(start).div_ceil(page_bytes)` → `u64::from(start) / page_bytes` | `periph::extmem::tests::an_invalidation_drops_the_pages_of_the_whole_entries_it_covers_in_either_window` |
| 6 | `E`: `(first..end / page_bytes)` → `(first..end.div_ceil(page_bytes))` | the same test |
| 7 | `E`: the two sets cleared whenever `lines == 0`, whatever `addr` | `periph::extmem::tests::a_request_that_names_no_line_of_a_window_drops_nothing` |
| 8 | `E`: `[FLASH_DBUS_BASE, FLASH_IBUS_BASE]` → `[FLASH_DBUS_BASE]` | `periph::extmem::tests::an_invalidation_drops_the_pages_of_the_whole_entries_it_covers_in_either_window` |
| 9 | `E`: `const LINE_BYTES: u64 = 32;` → `1` | the same test |
| 10 | `E`, `flash_written`: `first_page..=last_page.min(FLASH_PAGES - 1)` → `first_page..=first_page.min(FLASH_PAGES - 1)` | `periph::extmem::tests::a_write_marks_the_pages_of_its_range_that_were_read_and_no_other` |
| 11 | `E`: a write of `ICACHE_CTRL` also clears the two sets | `periph::extmem::tests::switching_the_cache_off_and_on_keeps_its_lines` |
| 12 | `periph/spi_mem.rs`: `if self.flash.generation() != before` and the `&& let Some(…)` under it → `if let Some((first_page, last_page)) = cache_pages(&write)` | `periph::spi_mem::tests::a_command_that_changes_the_flash_tells_the_cache_which_pages_and_no_other_does`, `bus::tests::flash_written_before_a_window_read_it_or_left_as_it_was_is_not_reported` |
| 13 | `periph/spi_mem.rs`, `cache_pages`: `let last = start.saturating_add(len.saturating_sub(1));` → `let last = start;` | `periph::spi_mem::tests::the_pages_of_a_write_are_those_of_its_bytes_whatever_its_numbers` |
| 14 | `bus.rs`: the three lines of `if self.periph.extmem.window_read(page) {` → `let _ = page;` | `bus::tests::a_window_read_of_flash_programmed_since_it_was_read_is_reported_and_returns_the_flash` |
| 15 | `bus.rs`: remove the `if let Some(page) = stale { … }` block | the same test, `bus::tests::strict_mode_turns_a_window_read_that_may_differ_into_a_violation` |
| 16 | `periph/mod.rs`: `} => self.extmem.flash_written(first_page, last_page),` → `} => {}` | `bus::tests::a_window_read_of_flash_programmed_since_it_was_read_is_reported_and_returns_the_flash` |
| 17 | `periph/mod.rs`: `Event::CacheInvalidated => self.extmem.invalidate(&self.mmu),` → `Event::CacheInvalidated => {}` | `bus::tests::an_invalidation_of_the_page_ends_the_report_until_it_is_written_again` |
| 18 | `diag.rs`: remove `\| Diagnostic::StaleFlashWindow { .. }` from the strict set | `bus::tests::strict_mode_turns_a_window_read_that_may_differ_into_a_violation`, `diag::tests::strict_mode_stops_on_the_events_that_cannot_be_right_silently` |
| 19 | `diag.rs`: the key `(21, page, 0)` → `(21, vaddr, 0)` | `diag::tests::a_window_read_that_may_differ_is_listed_once_per_flash_page_and_says_what_to_do` |

Row 20, on the real ROM and Espressif's bootloader: mutation 17, then

```bash
cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t6-m20.log 2>&1; echo "exit=$?"
```

Expected: `exit=101`; `the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only` fails: its run reports a `StaleFlashWindow` at the second boot. A flash page read through a window before the `otadata` write is read again after it, and the invalidations the ROM and the bootloader issue on the way are what ends its stale state on an unmutated build.

- [ ] **Step 10: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(soc): a window read that may differ from the device is reported

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src crates/emu-cli docs/adr/0002-flash-model-and-cache-windows.md docs/registers-accepted-without-effect.md
```

---
## Task 7: The content of the flash leaves the machine; only the machine builds its chip

A front end that keeps the flash of a device between two runs needs two things from the machine, and no host I/O in it: the whole content as bytes, and whether the run changed it. This task adds both, and closes a door: `SocBus::new` and `Peripherals::new` took any `FlashDevice`, so a caller could build a bus around a chip whose size contradicts `SocConfig::flash`. Nothing outside the crate called them.

**Files:**
- Modify: `crates/soc-esp32c3/src/machine.rs` (`flash_contents`, `flash_modified_sectors`)
- Modify: `crates/soc-esp32c3/src/flash.rs` (`NorFlash::read` copies by slice)
- Modify: `crates/soc-esp32c3/src/bus.rs` (`SocBus::new` is `pub(crate)`, with a `compile_fail` doctest)
- Modify: `crates/soc-esp32c3/src/periph/mod.rs` (`Peripherals::new` is `pub(crate)`)

**Interfaces:**
- Consumes: `FlashDevice::{size, read, modified_sectors}` (Task 3), `Esp32C3::with_flash(rom, image, config) -> Result<Esp32C3, FlashImageError>`.
- Produces:
  - `pub fn Esp32C3::flash_contents(&self) -> Vec<u8>`: exactly `FlashConfig::size` bytes, erased bytes included; given back to `Esp32C3::with_flash` with the same configuration it builds a machine whose flash holds the same bytes.
  - `pub fn Esp32C3::flash_modified_sectors(&self) -> u32`: the 4 KiB sectors that hold a byte the run changed since the machine was built; 0 says the content is the one the machine was given. A reset does not restart the count.
  - `SocBus::new` and `Peripherals::new` are `pub(crate)`.

**Bounds.** The allocation of `flash_contents` is the size of the chip, a field of the configuration (at most 16 MiB, `MAX_FLASH_BYTES`), never a value the firmware wrote. `NorFlash::read` copies `min(held, out.len())` bytes and fills the rest.

The two doc examples take their `RomImage` as a parameter: `RomImage::from_flat` exists only under the `test-support` feature, and an example that called it would compile in `cargo test --workspace` (where another crate switches the feature on) and fail in `cargo test -p soc-esp32c3`. For the `compile_fail` example that would be worse than a failure: it would pass for the wrong reason, whatever the visibility of `SocBus::new`.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -625,6 +662,75 @@ mod tests {
         assert_eq!(m.drain_diagnostics(), []);
     }
 
+    /// Where two flash contents first differ, their common length when one is longer: a failed
+    /// comparison says an offset, not 16 MiB of bytes.
+    fn first_difference(left: &[u8], right: &[u8]) -> Option<usize> {
+        left.iter()
+            .zip(right)
+            .position(|(l, r)| l != r)
+            .or_else(|| (left.len() != right.len()).then_some(left.len().min(right.len())))
+    }
+
+    #[test]
+    fn the_content_of_the_flash_leaves_the_machine_whole_and_builds_the_same_flash_again() {
+        // A sector erased, four bytes programmed in it, and a program in another sector that
+        // changes no byte (0x00 AND 0x0F is 0x00): one sector was modified, not two.
+        let mut program = flash_erase_sector(0x9000);
+        program.extend(flash_program(0x9020, &[0xFE, 0xCA, 0xFF, 0x00]));
+        program.extend(flash_program(0x2000, &[0x0F]));
+        program.push(HANG);
+        let config = SocConfig::default();
+        let size = config.flash.size() as usize;
+        let image = vec![0u8; 0xA000];
+        let mut m = Esp32C3::with_flash(flat_rom(&program), image.clone(), &config).unwrap();
+        let before = m.flash_contents();
+        assert_eq!(before.len(), size, "the whole chip, not the image");
+        assert_eq!(before[..image.len()], image[..]);
+        assert!(before[image.len()..].iter().all(|&byte| byte == 0xFF));
+        assert_eq!(m.flash_modified_sectors(), 0);
+
+        m.run_until(Cycles(400));
+        let after = m.flash_contents();
+        let mut expected = before.clone();
+        expected[0x9000..0xA000].fill(0xFF);
+        expected[0x9020..0x9024].copy_from_slice(&[0xFE, 0xCA, 0xFF, 0x00]);
+        assert_eq!(
+            first_difference(&after, &expected),
+            None,
+            "the erase and the program, nothing else"
+        );
+        assert_eq!(m.flash_modified_sectors(), 1);
+        m.reset(ResetCause::Mwdt0Core);
+        assert_eq!(m.flash_modified_sectors(), 1, "the chip is not reset");
+        assert_eq!(first_difference(&m.flash_contents(), &expected), None);
+
+        // What left the machine is a content `with_flash` takes: the next machine starts from it.
+        let next = Esp32C3::with_flash(flat_rom(&[HANG]), after, &config).unwrap();
+        assert_eq!(first_difference(&next.flash_contents(), &expected), None);
+        assert_eq!(
+            next.flash_modified_sectors(),
+            0,
+            "a new run, nothing modified"
+        );
+    }
+
+    #[test]
+    fn a_machine_with_an_erased_flash_hands_over_a_chip_of_erased_bytes() {
+        let small = SocConfig {
+            flash: crate::flash::FlashConfig::new(
+                1 << 20,
+                crate::flash::JedecId::from_u24(0x9D_6014).unwrap(),
+            )
+            .unwrap(),
+            ..SocConfig::default()
+        };
+        let m = Esp32C3::new(flat_rom(&[HANG]), &small);
+        let content = m.flash_contents();
+        assert_eq!(content.len(), 1 << 20, "the size of the configuration");
+        assert!(content.iter().all(|&byte| byte == 0xFF));
+        assert_eq!(m.flash_modified_sectors(), 0);
+    }
+
     #[test]
     fn hang_instruction_is_what_the_assembler_produces() {
         assert_eq!(jal(0, 0), HANG);
````

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -772,6 +774,35 @@ mod tests {
         assert_eq!(bytes, [2, 0x4F, 0xFF, 0xFF, 0xFF, 0xFF]);
     }
 
+    #[test]
+    fn a_read_of_any_range_is_the_image_where_it_has_bytes_and_erased_elsewhere() {
+        let image: Vec<u8> = (0..=9).collect();
+        let flash = NorFlash::with_image(image.clone(), FlashConfig::default()).unwrap();
+        // (offset, length): inside the image, straddling its end, at its end, far past it, and
+        // the offsets no chip has.
+        for (offset, len) in [
+            (0u32, 10usize),
+            (0, 3),
+            (7, 3),
+            (7, 8),
+            (10, 4),
+            (11, 4),
+            (0, 0),
+            (10, 0),
+            (1 << 24, 4),
+            (u32::MAX, 4),
+        ] {
+            let mut bytes = vec![0x55u8; len];
+            flash.read(offset, &mut bytes);
+            let expected: Vec<u8> = (u64::from(offset)..)
+                .take(len)
+                .map(|at| usize::try_from(at).ok().and_then(|at| image.get(at)))
+                .map(|byte| byte.copied().unwrap_or(ERASED))
+                .collect();
+            assert_eq!(bytes, expected, "{offset:#x} + {len}");
+        }
+    }
+
     #[test]
     fn an_image_longer_than_the_flash_is_refused_and_one_that_fits_exactly_is_not() {
         let small = FlashConfig::new(MIN_FLASH_BYTES, id_with(0x14)).unwrap();
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 --lib > /tmp/t7-red.log 2>&1; echo "exit=$?"; grep -m2 "^error" /tmp/t7-red.log
```

Expected: `exit=101`; ``error[E0599]: no method named `flash_contents` found for struct `machine::Esp32C3` `` and the same for `flash_modified_sectors`. (`a_read_of_any_range_is_the_image_where_it_has_bytes_and_erased_elsewhere` holds a behaviour `NorFlash::read` already has: it passes before and after Step 3, and is there for the rewrite of that function.)

- [ ] **Step 3: The content and the modified state**

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -505,6 +505,43 @@ impl Esp32C3 {
         self.bus.periph.spi1.flash_writes_dropped()
     }
 
+    /// The whole content of the flash chip as it is now, erased bytes included: exactly
+    /// [`FlashConfig::size`](crate::flash::FlashConfig::size) bytes, at most 16 MiB whatever the
+    /// firmware did. It is what a caller keeps to let a later run go on from this one: given
+    /// back to [`Esp32C3::with_flash`] with the same configuration, it builds a machine whose
+    /// flash holds the same bytes. The machine performs no host I/O; storing the bytes is the
+    /// caller's business.
+    ///
+    /// ```
+    /// use soc_esp32c3::{Esp32C3, FlashImageError, RomImage, SocConfig};
+    /// /// The machine of the next run: it starts again from the ROM, on the flash `last` left.
+    /// fn next_run(
+    ///     rom: RomImage,
+    ///     last: &Esp32C3,
+    ///     config: &SocConfig,
+    /// ) -> Result<Esp32C3, FlashImageError> {
+    ///     Esp32C3::with_flash(rom, last.flash_contents(), config)
+    /// }
+    /// ```
+    #[must_use]
+    pub fn flash_contents(&self) -> Vec<u8> {
+        let flash = self.bus.periph.spi1.flash();
+        // INVARIANT: a `u32` fits in `usize` on every target the workspace builds for (32 bits
+        // and more); the fallback, an empty content, is never taken.
+        let mut content = vec![0u8; usize::try_from(flash.size()).unwrap_or(0)];
+        flash.read(0, &mut content);
+        content
+    }
+
+    /// How many 4 KiB sectors of the flash hold a byte the run changed since the machine was
+    /// built: 0 says the content is the one the machine was given. A program or an erase that
+    /// left every byte as it was counts for nothing, and a reset does not start the count again:
+    /// the chip is not reset with the rest.
+    #[must_use]
+    pub fn flash_modified_sectors(&self) -> u32 {
+        self.bus.periph.spi1.flash().modified_sectors()
+    }
+
     /// Diagnostics seen for the first time since the last call.
     pub fn drain_diagnostics(&mut self) -> Vec<Diagnostic> {
         self.bus.ctx.diag.drain()
````

````diff
--- a/crates/soc-esp32c3/src/flash.rs
+++ b/crates/soc-esp32c3/src/flash.rs
@@ -626,14 +626,16 @@ impl FlashDevice for NorFlash {
     }
 
     fn read(&self, offset: u32, out: &mut [u8]) {
-        let start = usize::try_from(offset).unwrap_or(usize::MAX);
-        for (i, byte) in out.iter_mut().enumerate() {
-            *byte = start
-                .checked_add(i)
-                .and_then(|index| self.image.get(index))
-                .copied()
-                .unwrap_or(ERASED);
-        }
+        // The part of `out` the image holds is copied in one piece (the whole chip is read this
+        // way when its content leaves the machine); the rest reads erased.
+        let held = usize::try_from(offset)
+            .ok()
+            .and_then(|start| self.image.get(start..))
+            .unwrap_or_default();
+        let (copied, erased) = out.split_at_mut(held.len().min(out.len()));
+        // INVARIANT: `copied` is no longer than `held`, by the `min` above.
+        copied.copy_from_slice(&held[..copied.len()]);
+        erased.fill(ERASED);
     }
 
     fn execute(
````

- [ ] **Step 4: Only the crate builds a bus**

````diff
--- a/crates/soc-esp32c3/src/bus.rs
+++ b/crates/soc-esp32c3/src/bus.rs
@@ -100,8 +100,22 @@ const fn size_mask(size: AccessSize) -> u32 {
 }
 
 impl SocBus {
+    /// The bus of a chip whose flash is `flash`.
+    ///
+    /// Only this crate can call it: [`Esp32C3::new`](crate::Esp32C3::new) and
+    /// [`Esp32C3::with_flash`](crate::Esp32C3::with_flash) build the chip from
+    /// [`SocConfig::flash`], so a machine never holds a flash whose size or id is another than
+    /// the one its configuration states. A caller outside the crate hands over bytes, not a chip;
+    /// the only thing that does not compile below is the call:
+    ///
+    /// ```compile_fail
+    /// use soc_esp32c3::{FlashDevice, RomImage, SocConfig};
+    /// fn a_bus_around_my_own_chip(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) {
+    ///     let _ = soc_esp32c3::bus::SocBus::new(rom, flash, config);
+    /// }
+    /// ```
     #[must_use]
-    pub fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus {
+    pub(crate) fn new(rom: RomImage, flash: Box<dyn FlashDevice>, config: &SocConfig) -> SocBus {
         let mut ctx = Ctx::new(config.strict_mmio);
         let periph = Peripherals::new(config, flash, &mut ctx);
         SocBus {
````

````diff
--- a/crates/soc-esp32c3/src/periph/mod.rs
+++ b/crates/soc-esp32c3/src/periph/mod.rs
@@ -428,8 +428,15 @@ impl Peripherals {
     /// The peripherals after a power-on, at the device time of `ctx`: a device that has a first
     /// deadline schedules it there. The reset values arm the flash-boot protection of main
     /// watchdog 0 and of the RTC watchdog when the chip is strapped to boot from flash.
+    ///
+    /// Only this crate can call it, for the reason [`SocBus::new`](crate::bus::SocBus) gives: the
+    /// chip is the one [`SocConfig::flash`] describes.
     #[must_use]
-    pub fn new(config: &SocConfig, flash: Box<dyn FlashDevice>, ctx: &mut Ctx) -> Peripherals {
+    pub(crate) fn new(
+        config: &SocConfig,
+        flash: Box<dyn FlashDevice>,
+        ctx: &mut Ctx,
+    ) -> Peripherals {
         let flash_boot = config.strapping & STRAP_SPI_FAST_FLASH_BOOT != 0;
         let efuse = Efuse::new(config.chip_revision);
         let mut periph = Peripherals {
````

- [ ] **Step 5: Run the suite, the package alone, and the WebAssembly check**

```bash
cargo test --workspace > /tmp/t7.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t7.log
cargo test -p soc-esp32c3 --doc > /tmp/t7-doc.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t7-doc.log
cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 > /tmp/t7-wasm.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` and `899`; `exit=0` and `2` (the two doc examples, without the `test-support` feature); `exit=0`.

- [ ] **Step 6: Mutations**

| Mutation | Command | Test that fails |
|---|---|---|
| `bus.rs`: `pub(crate) fn new(rom: RomImage, …` → `pub fn new(rom: RomImage, …` | `cargo test -p soc-esp32c3 --doc` | the doctest of `bus::SocBus::new`: "Test compiled successfully, but it's marked `compile_fail`" |
| `machine.rs`, `flash_contents`: remove `flash.read(0, &mut content);` | `cargo test -p soc-esp32c3 --lib` | `machine::tests::the_content_of_the_flash_leaves_the_machine_whole_and_builds_the_same_flash_again`, `machine::tests::a_machine_with_an_erased_flash_hands_over_a_chip_of_erased_bytes` |
| `machine.rs`: `flash.read(0, &mut content);` → `flash.read(1, &mut content);` | the same | `machine::tests::the_content_of_the_flash_leaves_the_machine_whole_and_builds_the_same_flash_again` |
| `machine.rs`, `flash_modified_sectors`: return `flash_writes_dropped()` (cast) instead of `modified_sectors()` | the same | the same test |
| `flash.rs`, `read`: `erased.fill(ERASED);` → `erased.fill(0);` | the same | `flash::tests::a_read_of_any_range_is_the_image_where_it_has_bytes_and_erased_elsewhere`, `machine::tests::a_machine_with_an_erased_flash_hands_over_a_chip_of_erased_bytes` |
| `flash.rs`, `read`: `held.len().min(out.len())` → `held.len().min(out.len()).saturating_sub(1)` | the same | `flash::tests::a_read_of_any_range_is_the_image_where_it_has_bytes_and_erased_elsewhere` |

- [ ] **Step 7: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(soc): the content of the flash leaves the machine; only the machine builds its chip

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3/src
```

---
## Task 8: The flash file of a device: locked, read whole, replaced by a rename

The mechanism, with no policy yet: a module of the CLI that turns a file into the bytes a machine is built from and back. Nothing uses it until Task 10. The file is untrusted input: whatever is at the path, the module refuses it by name or reads exactly the flash, and never more than one byte past it.

**Files:**
- Create: `crates/emu-cli/src/flash_state.rs`
- Modify: `crates/emu-cli/src/lib.rs` (`pub mod flash_state;`)

**Interfaces:**
- Consumes: `crate::term::{hex, sanitize}`, `soc_esp32c3::flash::ERASED`, `sha2`, `thiserror`, `std::fs::File::try_lock`.
- Produces, in `emu_cli::flash_state`:
  - `pub const LOCK_SUFFIX: &str = ".lock";`
  - `pub enum FlashStateError { NoFileName, LockCreate, InUse, Lock, NotAFile, Read, WrongSize { path, len: u64, size: u32 }, Changed, Write { path, action, target, detail, size } }`, each message "cause; remedy", paths with their control characters escaped.
  - `pub fn content_sha256(image: &[u8], size: u32) -> String`: the SHA-256 of a flash of `size` bytes holding `image` from offset 0 and erased after it, which is what `sha256sum` says of the flash file of that content.
  - `pub struct FlashState` with `pub fn open(path: &Path, size: u32) -> Result<FlashState, FlashStateError>` (creates `<path>.lock` if absent and locks it for the life of the value; reads and changes nothing of the flash file), `pub fn shown(&self) -> &str`, `pub fn exists(&self) -> Result<bool, FlashStateError>` (a file that exists must be a regular file of exactly `size` bytes; a link is not followed), `pub fn load(&self) -> Result<Vec<u8>, FlashStateError>`, `pub fn save(&self, content: &[u8]) -> Result<(), FlashStateError>` (temporary file `.<name>.<pid>.tmp` in the same directory, `create_new`, `sync_all`, then `rename`).

**Bounds on untrusted input.** The size of the file is judged on its metadata before a byte is read (`check`); `read_exactly` reads through `take(size + 1)`, so a file that grows after the look costs at most one byte more than the flash; `size` is the flash size of a device profile, 16 MiB at most. `content_sha256` feeds the erased part 4 KiB at a time, at most `size / 4096` rounds.

**Why the lock is on a sibling file.** The flash file may not exist yet, and it is replaced by a rename at the end of the run: a lock held on it would be a lock on the file that is no longer at the path. `<path>.lock` is created once and never removed: removing it would let a run that already opened it and a run that creates it anew both hold "the" lock.

- [ ] **Step 1: Write the failing tests**

Add the module to `crates/emu-cli/src/lib.rs`:

````diff
--- a/crates/emu-cli/src/lib.rs
+++ b/crates/emu-cli/src/lib.rs
@@ -1,6 +1,7 @@
 //! Library side of the inkemu CLI, so integration tests can drive the same code as the binary.
 pub mod exit;
 pub mod flash_file;
+pub mod flash_state;
 pub mod inspect;
 pub mod reader;
 pub mod rom_store;
````

Create `crates/emu-cli/src/flash_state.rs` with its tests only (two of them are `#[cfg(unix)]`: a symbolic link, and a directory removed under the run):

````rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A flash small enough for a test to write many times; the module knows no minimum.
    const SIZE: u32 = 8192;

    fn content(seed: u8) -> Vec<u8> {
        (0..SIZE).map(|i| seed ^ i.to_le_bytes()[0]).collect()
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn opening_creates_the_lock_file_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        assert_eq!(names(dir.path()), ["x4.bin.lock"]);
        assert_eq!(std::fs::read(dir.path().join("x4.bin.lock")).unwrap(), b"");
        assert!(!state.exists().unwrap(), "no flash file yet: a first run");
        assert!(state.shown().ends_with("x4.bin"));
    }

    #[test]
    fn what_is_saved_is_what_the_next_run_loads_and_the_file_is_the_content_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        {
            let state = FlashState::open(&path, SIZE).unwrap();
            state.save(&content(1)).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), content(1), "no header");
            assert_eq!(
                names(dir.path()),
                ["x4.bin", "x4.bin.lock"],
                "no temporary file is left"
            );
            assert!(state.exists().unwrap());
            assert_eq!(state.load().unwrap(), content(1));
            state.save(&content(2)).unwrap();
        }
        let next = FlashState::open(&path, SIZE).unwrap();
        assert!(next.exists().unwrap());
        assert_eq!(next.load().unwrap(), content(2), "the last one saved");
        assert_eq!(
            names(dir.path()),
            ["x4.bin", "x4.bin.lock"],
            "the lock file stays"
        );
    }

    #[test]
    fn a_second_run_on_the_same_file_is_refused_until_the_first_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let first = FlashState::open(&path, SIZE).unwrap();
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::InUse { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("x4.bin") && text.contains("x4.bin.lock") && text.contains("; wait for"),
            "{text}"
        );
        // Another file of the same directory is another device: no conflict.
        FlashState::open(&dir.path().join("x3.bin"), SIZE).unwrap();
        // The same file reached by another spelling of its path is the same lock.
        let other_spelling = dir.path().join(".").join("x4.bin");
        assert!(matches!(
            FlashState::open(&other_spelling, SIZE),
            Err(FlashStateError::InUse { .. })
        ));
        drop(first);
        FlashState::open(&path, SIZE).unwrap();
    }

    #[test]
    fn a_file_of_another_size_is_refused_with_both_sizes_and_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        for len in [0, 1, SIZE - 1, SIZE + 1, 2 * SIZE] {
            let bytes = vec![0xA5u8; len as usize];
            std::fs::write(&path, &bytes).unwrap();
            for err in [state.exists().unwrap_err(), state.load().unwrap_err()] {
                assert!(
                    matches!(err, FlashStateError::WrongSize { len: found, size: SIZE, .. } if found == u64::from(len)),
                    "{len}: {err}"
                );
                let text = err.to_string();
                assert!(
                    text.contains(&format!("is {len} bytes"))
                        && text.contains("8192 bytes")
                        && text.contains("never padded or cut")
                        && text.contains("; use it with"),
                    "{text}"
                );
            }
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{len}: untouched");
        }
    }

    #[test]
    fn a_huge_file_is_refused_on_its_size_without_being_read() {
        // A sparse file far larger than any flash: judged from its metadata.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        File::create(&path).unwrap().set_len(1 << 40).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        assert!(matches!(
            state.load(),
            Err(FlashStateError::WrongSize { len, .. }) if len == 1 << 40
        ));
    }

    #[test]
    fn reading_stops_one_byte_past_the_size_and_only_the_exact_size_is_a_content() {
        assert_eq!(read_exactly(std::io::repeat(0xFF), 16).unwrap(), Err(17));
        assert_eq!(read_exactly(&[7u8; 16][..], 16).unwrap(), Ok(vec![7; 16]));
        assert_eq!(read_exactly(&[7u8; 15][..], 16).unwrap(), Err(15));
        assert_eq!(read_exactly(&[7u8; 17][..], 16).unwrap(), Err(17));
        assert_eq!(read_exactly(std::io::empty(), 0).unwrap(), Ok(Vec::new()));
        let text = FlashStateError::Changed {
            path: "x4.bin".into(),
            size: 16,
            found: "more".into(),
        }
        .to_string();
        assert!(
            text.contains("held 16 bytes, then more") && text.contains("; make sure"),
            "{text}"
        );
    }

    #[test]
    fn what_is_not_a_regular_file_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        std::fs::create_dir(&path).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        for err in [state.exists().unwrap_err(), state.load().unwrap_err()] {
            assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
            assert!(err.to_string().contains("; give the path"), "{err}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed_and_what_it_points_to_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("precious.bin");
        std::fs::write(&target, content(9)).unwrap();
        let path = dir.path().join("x4.bin");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let state = FlashState::open(&path, SIZE).unwrap();
        let err = state.exists().unwrap_err();
        assert!(matches!(err, FlashStateError::NotAFile { .. }), "{err}");
        assert!(err.to_string().contains("a link is not followed"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), content(9));
    }

    #[test]
    fn a_path_without_a_file_name_or_in_a_missing_directory_is_refused_before_the_run() {
        for path in ["", "/", ".."] {
            let err = FlashState::open(Path::new(path), SIZE).unwrap_err();
            assert!(
                matches!(err, FlashStateError::NoFileName { .. }),
                "{path:?}: {err}"
            );
            assert!(err.to_string().contains("; give the path of a file"));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("x4.bin");
        let err = FlashState::open(&path, SIZE).unwrap_err();
        assert!(matches!(err, FlashStateError::LockCreate { .. }), "{err}");
        let text = err.to_string();
        assert!(
            text.contains("x4.bin.lock") && text.contains("; check that the directory exists"),
            "{text}"
        );
        assert_eq!(names(dir.path()), [""; 0], "nothing was created");
    }

    // Unix: a directory that holds an open file (the lock) can be removed there, not everywhere.
    #[cfg(unix)]
    #[test]
    fn a_directory_that_vanished_fails_the_save_by_name_and_says_what_is_lost() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("flash");
        std::fs::create_dir(&inner).unwrap();
        let state = FlashState::open(&inner.join("x4.bin"), SIZE).unwrap();
        std::fs::remove_dir_all(&inner).unwrap();
        let err = state.save(&content(1)).unwrap_err();
        assert!(
            matches!(
                err,
                FlashStateError::Write {
                    action: "create",
                    size: SIZE,
                    ..
                }
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("cannot write the flash file")
                && text.contains("x4.bin")
                && text.contains("is lost")
                && text.contains("; check that the directory still exists"),
            "{text}"
        );
    }

    #[test]
    fn a_save_that_cannot_take_the_name_leaves_the_old_content_and_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        // A directory took the place of the flash file during the run: the rename fails.
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("kept"), b"old").unwrap();
        let err = state.save(&content(1)).unwrap_err();
        assert!(
            matches!(
                err,
                FlashStateError::Write {
                    action: "move into place",
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(std::fs::read(path.join("kept")).unwrap(), b"old");
        assert_eq!(
            names(dir.path()),
            ["x4.bin", "x4.bin.lock"],
            "no temporary file"
        );
    }

    #[test]
    fn a_leftover_at_the_temporary_name_is_replaced_and_a_link_planted_there_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        let temporary = dir
            .path()
            .join(format!(".x4.bin.{}.tmp", std::process::id()));
        assert_eq!(state.temporary, temporary);
        std::fs::write(&temporary, b"left by a killed run").unwrap();
        state.save(&content(1)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), content(1));
        #[cfg(unix)]
        {
            let victim = dir.path().join("victim");
            std::fs::write(&victim, b"untouched").unwrap();
            std::os::unix::fs::symlink(&victim, &temporary).unwrap();
            state.save(&content(2)).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), content(2));
            assert_eq!(std::fs::read(&victim).unwrap(), b"untouched");
        }
        assert!(!temporary.exists());
    }

    #[test]
    fn a_file_cut_between_the_look_and_the_read_is_refused_not_padded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x4.bin");
        let state = FlashState::open(&path, SIZE).unwrap();
        state.save(&content(1)).unwrap();
        assert!(state.exists().unwrap());
        OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(u64::from(SIZE) - 1)
            .unwrap();
        assert!(matches!(
            state.load(),
            Err(FlashStateError::WrongSize { .. })
        ));
        std::fs::remove_file(&path).unwrap();
        let err = state.load().unwrap_err();
        assert!(matches!(err, FlashStateError::Read { .. }), "{err}");
        assert!(err.to_string().contains("; check its read permissions"));
    }

    #[test]
    fn the_hash_of_a_content_is_the_hash_of_the_flash_it_makes() {
        let image = content(3);
        let half = &image[..4096];
        let mut padded = half.to_vec();
        padded.resize(SIZE as usize, 0xFF);
        assert_eq!(
            content_sha256(half, SIZE),
            hex(&Sha256::digest(&padded)),
            "erased after the image"
        );
        assert_eq!(content_sha256(&image, SIZE), hex(&Sha256::digest(&image)));
        assert_eq!(
            content_sha256(&[], SIZE),
            hex(&Sha256::digest(vec![0xFFu8; SIZE as usize])),
            "an erased flash"
        );
        assert_eq!(
            content_sha256(&image, 100),
            hex(&Sha256::digest(&image[..100])),
            "what is past the flash is not in it"
        );
        // An erased part that is no whole number of rounds, and none at all.
        assert_eq!(
            content_sha256(&[1, 2, 3], 5000),
            hex(&Sha256::digest(
                [&[1u8, 2, 3][..], &[0xFF; 4997][..]].concat()
            ))
        );
        assert_eq!(content_sha256(&[], 0), hex(&Sha256::digest([])));
        // What `sha256sum` says of an erased 16 MiB flash file.
        assert_eq!(
            content_sha256(&[], 16 << 20),
            "dffab0dd410657cb30c7b2fd7f2586a4792e8472e58882b3532581f8111a646d"
        );
    }

    #[test]
    fn a_path_with_control_characters_is_shown_escaped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("\u{1b}[31mred.bin");
        let err = FlashState::open(&path, SIZE).unwrap_err().to_string();
        assert!(!err.contains('\u{1b}'), "{err:?}");
        assert!(err.contains("\\u{1b}[31mred.bin"), "{err}");
    }
}
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p emu-cli --lib flash_state > /tmp/t8-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t8-red.log
```

Expected: `exit=101`; the crate does not compile: ``cannot find type `FlashState` in this scope``, ``cannot find function `content_sha256` in this scope``, ``cannot find type `FlashStateError` in this scope``.

- [ ] **Step 3: The module**

Put this above the tests:

````rust
//! The flash file of a device (`inkemu run --flash-file`): the content of its flash chip, kept
//! from one run to the next (spec §5.1).
//!
//! The file is the chip as bytes and nothing else: no header, exactly the size of the flash. A
//! run reads it when it starts and replaces it when it ends; a lock file beside it keeps two runs
//! from using it at the same time. The machine knows nothing of it (it performs no host I/O):
//! this module turns a file into the bytes a machine is built from, and the bytes a machine hands
//! over into a file.
use crate::term::{hex, sanitize};
use sha2::{Digest, Sha256};
use soc_esp32c3::flash::ERASED;
use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// What is appended to the name of a flash file to name its lock file.
pub const LOCK_SUFFIX: &str = ".lock";

/// Why a flash file cannot be used, read or written. The messages state the cause, then the
/// remedy; the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum FlashStateError {
    #[error("the flash file path {path} names no file; give the path of a file, like flash-x4.bin")]
    NoFileName { path: String },
    #[error(
        "cannot create the lock file {lock} beside the flash file {path}: {detail}; check that the directory exists and is writable, or give a path in one that is"
    )]
    LockCreate {
        path: String,
        lock: String,
        detail: String,
    },
    #[error(
        "the flash file {path} is in use by another inkemu run, which holds the lock {lock}; wait for that run to end, or give this run another --flash-file"
    )]
    InUse { path: String, lock: String },
    #[error(
        "cannot lock the flash file {path} with {lock}: {detail}; give a path on a filesystem that supports file locks"
    )]
    Lock {
        path: String,
        lock: String,
        detail: String,
    },
    #[error(
        "the flash file {path} is not a regular file (a link is not followed: the file is replaced when the run ends, not written through); give the path of the file itself, or a new path"
    )]
    NotAFile { path: String },
    #[error("cannot read the flash file {path}: {detail}; check its read permissions")]
    Read { path: String, detail: String },
    #[error(
        "the flash file {path} is {len} bytes and the flash of the device is {size} bytes: the file is not the flash of this device, and it is never padded or cut; use it with the device it was written for, pass a firmware image with --flash instead, or give a new path"
    )]
    WrongSize { path: String, len: u64, size: u32 },
    #[error(
        "the flash file {path} changed while it was being read: it held {size} bytes, then {found}; make sure nothing else writes it, then run again"
    )]
    Changed {
        path: String,
        size: u32,
        found: String,
    },
    #[error(
        "cannot write the flash file {path}: cannot {action} {target}: {detail}; what this run wrote to the flash is lost; check that the directory still exists, is writable and has {size} bytes free, then run again"
    )]
    Write {
        path: String,
        action: &'static str,
        target: String,
        detail: String,
        size: u32,
    },
}

fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

/// The SHA-256, in hexadecimal, of a flash of `size` bytes that holds `image` from offset 0 and is
/// erased after it: what `sha256sum` says of the flash file of that content. Bytes of `image`
/// past `size` are not part of the flash and are left out.
///
/// The erased part is fed 4 KiB at a time: at most `size / 4096` rounds, and `size` is the flash
/// size of a device profile, 16 MiB at most.
#[must_use]
pub fn content_sha256(image: &[u8], size: u32) -> String {
    const ERASED_CHUNK: [u8; 4096] = [ERASED; 4096];
    // INVARIANT: a `u32` fits in `usize` on the hosts the CLI builds for; on one where it did
    // not, the whole image would be hashed and nothing added.
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    let held = image.get(..size).unwrap_or(image);
    let mut hasher = Sha256::new();
    hasher.update(held);
    let mut erased = size.saturating_sub(held.len());
    while erased > 0 {
        let round = ERASED_CHUNK.get(..erased).unwrap_or(&ERASED_CHUNK);
        hasher.update(round);
        erased -= round.len();
    }
    hex(&hasher.finalize())
}

/// Reads `source` to its end when it holds exactly `size` bytes; `Err(found)` says how many it
/// held otherwise. Never more than `size + 1` bytes are read, whatever the source is.
fn read_exactly(source: impl Read, size: u32) -> std::io::Result<Result<Vec<u8>, u64>> {
    let mut bytes = Vec::new();
    source.take(u64::from(size) + 1).read_to_end(&mut bytes)?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    Ok(if len == u64::from(size) {
        Ok(bytes)
    } else {
        Err(len)
    })
}

/// The flash file of one run, locked against other runs for as long as this value lives.
#[derive(Debug)]
pub struct FlashState {
    path: PathBuf,
    /// The file the content is written to before it takes the name of the flash file.
    temporary: PathBuf,
    shown: String,
    size: u32,
    /// The lock file, locked. Closing it releases the lock: at the end of the run, or when the
    /// process dies, whatever kills it. The file itself is never removed: removing it would let
    /// a run that already opened it and a run that creates it anew both hold "the" lock.
    _lock: File,
}

impl FlashState {
    /// Takes the flash file at `path` for a run of a device whose flash is `size` bytes: creates
    /// the lock file beside it if there is none, and locks it. Nothing of the flash file itself
    /// is read, created or changed.
    ///
    /// # Errors
    /// [`FlashStateError`] when the path names no file, the lock file cannot be created, or
    /// another run holds the lock.
    pub fn open(path: &Path, size: u32) -> Result<FlashState, FlashStateError> {
        let shown_path = shown(path);
        let Some(name) = path.file_name() else {
            return Err(FlashStateError::NoFileName { path: shown_path });
        };
        let sibling = |prefix: &str, suffix: &str| {
            let mut sibling = OsString::from(prefix);
            sibling.push(name);
            sibling.push(suffix);
            path.with_file_name(sibling)
        };
        let lock_path = sibling("", LOCK_SUFFIX);
        let shown_lock = shown(&lock_path);
        // Opened for writing so that the directory is known to be writable now, not at the end
        // of the run; nothing is ever written to it, and an existing one is not truncated.
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| FlashStateError::LockCreate {
                path: shown_path.clone(),
                lock: shown_lock.clone(),
                detail: sanitize(&e.to_string()),
            })?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(FlashStateError::InUse {
                    path: shown_path,
                    lock: shown_lock,
                });
            }
            Err(TryLockError::Error(e)) => {
                return Err(FlashStateError::Lock {
                    path: shown_path,
                    lock: shown_lock,
                    detail: sanitize(&e.to_string()),
                });
            }
        }
        Ok(FlashState {
            temporary: sibling(".", &format!(".{}.tmp", std::process::id())),
            path: path.to_path_buf(),
            shown: shown_path,
            size,
            _lock: lock,
        })
    }

    /// The path of the flash file, with its control characters escaped: for messages.
    #[must_use]
    pub fn shown(&self) -> &str {
        &self.shown
    }

    /// Whether the flash file exists. One that exists must be a flash of the device: a regular
    /// file of exactly the size of its flash.
    ///
    /// # Errors
    /// [`FlashStateError`] when what is at the path is not a regular file (a link is not
    /// followed), has another size, or cannot be looked at.
    pub fn exists(&self) -> Result<bool, FlashStateError> {
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => self.check(&metadata).map(|()| true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(self.read_error(&e)),
        }
    }

    /// The content of the flash file: exactly the bytes of the flash of the device. The size is
    /// judged before anything is read, and no more than one byte past it is ever read.
    ///
    /// # Errors
    /// [`FlashStateError`] when the file cannot be read, is not a regular file of the size of
    /// the flash, or changes while it is read.
    pub fn load(&self) -> Result<Vec<u8>, FlashStateError> {
        let file = File::open(&self.path).map_err(|e| self.read_error(&e))?;
        // What was opened is judged, not what the path named a moment ago.
        self.check(&file.metadata().map_err(|e| self.read_error(&e))?)?;
        read_exactly(file, self.size)
            .map_err(|e| self.read_error(&e))?
            .map_err(|found| FlashStateError::Changed {
                path: self.shown.clone(),
                size: self.size,
                found: if found > u64::from(self.size) {
                    "more".to_owned()
                } else {
                    format!("{found}")
                },
            })
    }

    /// Replaces the flash file with `content`, the whole flash of the device. The bytes go to a
    /// temporary file in the same directory, are flushed to the disk, and only then take the name
    /// of the flash file: whatever interrupts this, the path names the old content or the new
    /// one, never a part of either.
    ///
    /// # Errors
    /// [`FlashStateError::Write`], which says what could not be done to which file; the flash
    /// file is then the one of before, and no temporary file is left.
    pub fn save(&self, content: &[u8]) -> Result<(), FlashStateError> {
        let failed = |action: &'static str, target: &Path, e: &std::io::Error| {
            // Best effort: the failure being reported is the one worth knowing.
            let _ = std::fs::remove_file(&self.temporary);
            FlashStateError::Write {
                path: self.shown.clone(),
                action,
                target: shown(target),
                detail: sanitize(&e.to_string()),
                size: self.size,
            }
        };
        // `create_new` never follows a link and never reuses a file planted at the name. A
        // leftover of a killed run is removed (a link itself, never its target), once.
        let create = || {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.temporary)
        };
        let mut file = match create() {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                std::fs::remove_file(&self.temporary)
                    .map_err(|e| failed("remove", &self.temporary, &e))?;
                create()
            }
            other => other,
        }
        .map_err(|e| failed("create", &self.temporary, &e))?;
        file.write_all(content)
            .and_then(|()| file.sync_all())
            .map_err(|e| failed("write", &self.temporary, &e))?;
        drop(file);
        std::fs::rename(&self.temporary, &self.path)
            .map_err(|e| failed("move into place", &self.path, &e))
    }

    fn check(&self, metadata: &std::fs::Metadata) -> Result<(), FlashStateError> {
        if !metadata.is_file() {
            return Err(FlashStateError::NotAFile {
                path: self.shown.clone(),
            });
        }
        if metadata.len() != u64::from(self.size) {
            return Err(FlashStateError::WrongSize {
                path: self.shown.clone(),
                len: metadata.len(),
                size: self.size,
            });
        }
        Ok(())
    }

    fn read_error(&self, error: &std::io::Error) -> FlashStateError {
        FlashStateError::Read {
            path: self.shown.clone(),
            detail: sanitize(&error.to_string()),
        }
    }
}
````

- [ ] **Step 4: Run the suite**

```bash
cargo test -p emu-cli --lib flash_state > /tmp/t8-unit.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t8-unit.log
cargo test --workspace > /tmp/t8.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t8.log
```

Expected: `exit=0` and `15`; `exit=0` and `914`.

- [ ] **Step 5: Mutations**

Each row: apply the change to `flash_state.rs`, run `cargo test -p emu-cli --lib flash_state`, see the test fail, undo.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `match lock.try_lock() {` → `match Ok::<(), TryLockError>(()) {` | `a_second_run_on_the_same_file_is_refused_until_the_first_ends` |
| 2 | `if metadata.len() != u64::from(self.size) {` → `>` | `a_file_of_another_size_is_refused_with_both_sizes_and_left_as_it_is` |
| 3 | `.create_new(true)` → `.create(true).truncate(true)` | `a_leftover_at_the_temporary_name_is_replaced_and_a_link_planted_there_is_not_followed` |
| 4 | `std::fs::rename(&self.temporary, &self.path)` → `std::fs::copy(…).map(\|_\| ())` | `what_is_saved_is_what_the_next_run_loads_and_the_file_is_the_content_alone` |
| 5 | in `failed`, remove `let _ = std::fs::remove_file(&self.temporary);` | `a_save_that_cannot_take_the_name_leaves_the_old_content_and_no_temporary_file` |
| 6 | `std::fs::symlink_metadata(&self.path)` → `std::fs::metadata(&self.path)` | `a_link_is_not_followed_and_what_it_points_to_is_never_written` |
| 7 | `source.take(u64::from(size) + 1)` → `source.take(u64::from(size))` | `reading_stops_one_byte_past_the_size_and_only_the_exact_size_is_a_content` |
| 8 | in `load`, remove the `self.check(&file.metadata()…)?;` line | `a_huge_file_is_refused_on_its_size_without_being_read`, `what_is_not_a_regular_file_is_refused_by_name` |
| 9 | `let mut erased = size.saturating_sub(held.len());` → `0` | `the_hash_of_a_content_is_the_hash_of_the_flash_it_makes` |
| 10 | `let held = image.get(..size).unwrap_or(image);` → `let held = image;` | the same test |
| 11 | the lock opened at `path` instead of `&lock_path` | `opening_creates_the_lock_file_and_nothing_else` |
| 12 | `temporary: sibling(".", &format!(".{}.tmp", std::process::id())),` → `sibling("", ".tmp")` | `a_leftover_at_the_temporary_name_is_replaced_and_a_link_planted_there_is_not_followed` |
| 13 | in `exists`, `NotFound => Ok(false)` → `Ok(true)` | `opening_creates_the_lock_file_and_nothing_else` |
| 14 | `file.write_all(content)` → `file.write_all(&content[..content.len() / 2])` | `what_is_saved_is_what_the_next_run_loads_and_the_file_is_the_content_alone` |
| 15 | in `read_exactly`, `if len == u64::from(size) {` → `<=` | `reading_stops_one_byte_past_the_size_and_only_the_exact_size_is_a_content` |

One line has no test and cannot have one here: `file.sync_all()`. Its absence shows only when the host loses power between the write and the rename. It is listed in the hand-over note.

- [ ] **Step 6: Format, lint, commit**

```bash
/usr/bin/git add crates/emu-cli/src/flash_state.rs
/usr/bin/git commit -m "feat(cli): the flash file of a device: locked, read whole, replaced by a rename

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-cli/src/flash_state.rs crates/emu-cli/src/lib.rs
```

---
## Task 9: What a run starts from and what becomes of its flash when it ends

The policy, over the mechanism of Task 8 and still with no option of the CLI: one type decides, before any machine exists, what flash a run starts from, and says when the run ends what became of it. It is tested with a real machine that runs a few instructions, in a temporary directory.

**Files:**
- Modify: `crates/emu-cli/src/flash_state.rs` (`RunFlash`, `FlashStateError::BothGiven`)
- Modify: `crates/emu-cli/src/run.rs` (`RunEnd::exit_code_without_its_flash`)
- Modify: `crates/emu-cli/src/exit.rs` (its test)

**Interfaces:**
- Consumes: `FlashState::{open, exists, load, save, shown}`, `content_sha256` (Task 8); `Esp32C3::{flash_contents, flash_modified_sectors}` (Task 7); `RunEnd`, `EXIT_ERROR`, `EXIT_STRICT_MMIO`; `test_support::flash_program` (Task 3).
- Produces:
  - `FlashStateError::BothGiven { path: String }`.
  - `pub struct RunFlash` with
    - `pub fn begin(flash_file: Option<&Path>, reset: bool, seed: Option<Vec<u8>>, size: u32) -> Result<(RunFlash, Option<Vec<u8>>), FlashStateError>`: the second value is the content to build the machine from, `None` for an erased flash;
    - `pub const fn state(&self) -> Option<&FlashState>`, `pub const fn starts_from_the_file(&self) -> bool`;
    - `pub fn finish(&self, machine: &Esp32C3, log: &mut dyn Write) -> bool`: `false` when the flash file had to be written and could not be.
  - `pub const fn RunEnd::exit_code_without_its_flash(self) -> u8`: 3 for `StrictMmio`, 2 for every other end.
  - The lines `finish` writes, in this order:
    - always: `inkemu: flash at start: sha256 <64 hex digits>`
    - no flash file, sectors modified: `inkemu: flash at end: N of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next`
    - the run started from the file and modified nothing: `inkemu: flash at end: not modified; <path> is left as it is`
    - the file was written: `inkemu: flash at end: sha256 <64 hex digits>; N of its 4 KiB sectors were modified; saved to <path>`
    - the file could not be written: `error: cannot write the flash file <path>: …; what this run wrote to the flash is lost; …`

**The rules `begin` holds.**

| Flash file | `seed` (`--flash`) | `reset` | The run starts from | At the end |
|---|---|---|---|---|
| none | any | — | the seed, or an erased flash | nothing is kept |
| does not exist | any | any | the seed, or an erased flash | the file is created |
| exists | none | no | the file | written if a sector was modified |
| exists | given | no | **refused** (`BothGiven`) | — |
| exists | any | yes | the seed, or an erased flash | the file is replaced, modified or not |
| exists, not a flash of the device | any | any | **refused** (`WrongSize`, `NotAFile`) | the file is left as it is |

The last row is stricter than "a reset replaces the file": a firmware image given to `--flash-file` by mistake must survive the remedy the user reaches for.

**The exit code of a failed save.** What the firmware wrote is lost, which is an error of the environment: 2, whatever ended the run, a limit or a first Ctrl-C (0) and an idle CPU (4) included. One end keeps its code, a `--strict-mmio` stop (3), as it does when the console fails: the option exists for CI and its code must not depend on the state of a directory. The message is printed in every case. No exit code is added.

Nothing the firmware wrote sizes anything here: the content is the size of the chip.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/emu-cli/src/flash_state.rs
+++ b/crates/emu-cli/src/flash_state.rs
@@ -638,4 +773,248 @@ mod tests {
         assert!(!err.contains('\u{1b}'), "{err:?}");
         assert!(err.contains("\\u{1b}[31mred.bin"), "{err}");
     }
+
+    /// The flash of the machines below: 1 MiB, the smallest the model takes, so that a test
+    /// writes and hashes little.
+    const MIB: u32 = 1 << 20;
+
+    /// A machine of a device with a 1 MiB flash that starts from `content` (`None`: erased) and
+    /// runs `program` to its end.
+    fn ran(content: Option<Vec<u8>>, program: &[u32]) -> Esp32C3 {
+        use soc_esp32c3::test_support::{HANG, flat_rom};
+        let config = soc_esp32c3::SocConfig {
+            flash: soc_esp32c3::FlashConfig::new(
+                MIB,
+                soc_esp32c3::JedecId::from_u24(0x9D_6014).unwrap(),
+            )
+            .unwrap(),
+            ..soc_esp32c3::SocConfig::default()
+        };
+        let mut program = program.to_vec();
+        program.push(HANG);
+        let rom = flat_rom(&program);
+        let mut machine = match content {
+            Some(content) => Esp32C3::with_flash(rom, content, &config).unwrap(),
+            None => Esp32C3::new(rom, &config),
+        };
+        machine.run_until(emu_core::Cycles(400));
+        machine
+    }
+
+    /// A program that changes one byte of the flash sector at 0x3000.
+    fn writing(byte: u8) -> Vec<u32> {
+        soc_esp32c3::test_support::flash_program(0x3000, &[byte])
+    }
+
+    fn finish(flash: &RunFlash, machine: &Esp32C3) -> (bool, String) {
+        let mut log = Vec::new();
+        let kept = flash.finish(machine, &mut log);
+        (kept, String::from_utf8(log).unwrap())
+    }
+
+    /// A seed shorter than the flash, as a firmware image is.
+    fn seed() -> Vec<u8> {
+        vec![0x5A; 0x4000]
+    }
+
+    #[test]
+    fn without_a_flash_file_the_run_starts_from_the_seed_and_says_what_it_does_not_keep() {
+        let (flash, content) = RunFlash::begin(None, false, Some(seed()), MIB).unwrap();
+        assert_eq!(content, Some(seed()));
+        assert!(flash.state().is_none() && !flash.starts_from_the_file());
+        let start = format!(
+            "inkemu: flash at start: sha256 {}\n",
+            content_sha256(&seed(), MIB)
+        );
+        // A run that writes nothing has nothing more to say.
+        let (kept, log) = finish(&flash, &ran(content.clone(), &[]));
+        assert!(kept);
+        assert_eq!(log, start);
+        let (kept, log) = finish(&flash, &ran(content, &writing(0x00)));
+        assert!(kept, "nothing was to be kept");
+        assert_eq!(
+            log,
+            format!(
+                "{start}inkemu: flash at end: 1 of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next\n"
+            )
+        );
+        // No seed: an erased flash, whose hash is the one of a file of erased bytes.
+        let (flash, content) = RunFlash::begin(None, false, None, MIB).unwrap();
+        assert_eq!(content, None);
+        let (_, log) = finish(&flash, &ran(None, &[]));
+        assert_eq!(
+            log,
+            format!(
+                "inkemu: flash at start: sha256 {}\n",
+                hex(&Sha256::digest(vec![0xFFu8; MIB as usize]))
+            )
+        );
+    }
+
+    #[test]
+    fn a_first_run_creates_the_flash_file_and_the_next_one_goes_on_from_it() {
+        let dir = tempfile::tempdir().unwrap();
+        let path = dir.path().join("x4.bin");
+        // First run: no file yet, the seed, a write.
+        let (flash, content) = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap();
+        assert_eq!(content, Some(seed()));
+        assert!(!flash.starts_from_the_file());
+        let machine = ran(content, &writing(0x00));
+        let (kept, log) = finish(&flash, &machine);
+        assert!(kept);
+        let first = std::fs::read(&path).unwrap();
+        assert_eq!(first.len(), MIB as usize, "the whole flash, not the seed");
+        assert_eq!(first[0x3000], 0x00, "what the run wrote");
+        assert_eq!(first[0x3001], 0x5A, "what it started from");
+        assert_eq!(first[0x4000], 0xFF, "erased past the seed");
+        let first_hash = hex(&Sha256::digest(&first));
+        assert!(
+            log.ends_with(&format!(
+                "inkemu: flash at end: sha256 {first_hash}; 1 of its 4 KiB sectors were modified; saved to {}\n",
+                sanitize(&path.display().to_string())
+            )),
+            "{log}"
+        );
+        drop(flash);
+
+        // Second run: from the file, and it writes nothing: the file is not touched.
+        let (flash, content) = RunFlash::begin(Some(&path), false, None, MIB).unwrap();
+        assert!(flash.starts_from_the_file());
+        assert_eq!(content.as_deref(), Some(&first[..]));
+        let before = identity(&path);
+        let (kept, log) = finish(&flash, &ran(content.clone(), &[]));
+        assert!(kept);
+        assert_eq!(
+            log,
+            format!(
+                "inkemu: flash at start: sha256 {first_hash}\ninkemu: flash at end: not modified; {} is left as it is\n",
+                sanitize(&path.display().to_string())
+            )
+        );
+        // A program that leaves every byte as it was (0x00 over 0x00) modified nothing either.
+        let (kept, log) = finish(&flash, &ran(content.clone(), &writing(0x00)));
+        assert!(kept && log.contains("not modified"), "{log}");
+        assert_eq!(std::fs::read(&path).unwrap(), first);
+        assert_eq!(identity(&path), before, "the file was not written again");
+        // A run that changes a byte: the file becomes the new content, what the first run wrote
+        // included.
+        let another_write = soc_esp32c3::test_support::flash_program(0x5000, &[0x0F]);
+        let (kept, log) = finish(&flash, &ran(content, &another_write));
+        assert!(kept && log.contains("; saved to "), "{log}");
+        let second = std::fs::read(&path).unwrap();
+        assert_eq!((second[0x3000], second[0x5000]), (0x00, 0x0F));
+        assert_ne!(identity(&path), before, "replaced, not written in place");
+    }
+
+    /// What tells a file that was replaced from one that was left alone: its inode where there
+    /// is one, its modification time elsewhere.
+    #[cfg(unix)]
+    fn identity(path: &Path) -> u64 {
+        use std::os::unix::fs::MetadataExt as _;
+        std::fs::metadata(path).unwrap().ino()
+    }
+    #[cfg(not(unix))]
+    fn identity(path: &Path) -> std::time::SystemTime {
+        std::fs::metadata(path).unwrap().modified().unwrap()
+    }
+
+    #[test]
+    fn a_seed_and_an_existing_flash_file_are_refused_unless_the_file_is_reset() {
+        let dir = tempfile::tempdir().unwrap();
+        let path = dir.path().join("x4.bin");
+        let kept_content = vec![0x11u8; MIB as usize];
+        std::fs::write(&path, &kept_content).unwrap();
+        let err = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap_err();
+        assert!(matches!(err, FlashStateError::BothGiven { .. }), "{err}");
+        let text = err.to_string();
+        assert!(
+            text.contains("both --flash and the flash file")
+                && text.contains("x4.bin")
+                && text.contains("; drop --flash")
+                && text.contains("--reset-flash"),
+            "{text}"
+        );
+        assert_eq!(std::fs::read(&path).unwrap(), kept_content, "untouched");
+
+        // The refusal released the lock. A reset starts from the seed and replaces the file,
+        // even when the run modified nothing.
+        let (flash, content) = RunFlash::begin(Some(&path), true, Some(seed()), MIB).unwrap();
+        assert_eq!(content, Some(seed()));
+        assert!(!flash.starts_from_the_file());
+        let (kept, log) = finish(&flash, &ran(content, &[]));
+        assert!(
+            kept && log.contains("0 of its 4 KiB sectors were modified; saved to"),
+            "{log}"
+        );
+        let replaced = std::fs::read(&path).unwrap();
+        assert_eq!(replaced[..0x4000], seed()[..]);
+        assert!(replaced[0x4000..].iter().all(|&byte| byte == 0xFF));
+        drop(flash);
+
+        // A reset without a seed starts from an erased flash.
+        let (flash, content) = RunFlash::begin(Some(&path), true, None, MIB).unwrap();
+        assert_eq!(content, None);
+        let (kept, _) = finish(&flash, &ran(None, &[]));
+        assert!(kept);
+        assert!(
+            std::fs::read(&path)
+                .unwrap()
+                .iter()
+                .all(|&byte| byte == 0xFF)
+        );
+    }
+
+    #[test]
+    fn a_reset_does_not_replace_a_file_that_is_not_a_flash_of_the_device() {
+        // A firmware image given to --flash-file by mistake must survive --reset-flash.
+        let dir = tempfile::tempdir().unwrap();
+        let path = dir.path().join("firmware.bin");
+        std::fs::write(&path, seed()).unwrap();
+        for reset in [false, true] {
+            let err = RunFlash::begin(Some(&path), reset, None, MIB).unwrap_err();
+            assert!(
+                matches!(err, FlashStateError::WrongSize { .. }),
+                "{reset}: {err}"
+            );
+        }
+        assert_eq!(std::fs::read(&path).unwrap(), seed());
+    }
+
+    #[cfg(unix)]
+    #[test]
+    fn a_flash_file_that_cannot_be_written_is_said_in_the_log_and_answered_by_no() {
+        let dir = tempfile::tempdir().unwrap();
+        let inner = dir.path().join("flash");
+        std::fs::create_dir(&inner).unwrap();
+        let (flash, content) =
+            RunFlash::begin(Some(&inner.join("x4.bin")), false, None, MIB).unwrap();
+        std::fs::remove_dir_all(&inner).unwrap();
+        let (kept, log) = finish(&flash, &ran(content, &writing(0x00)));
+        assert!(!kept);
+        assert!(
+            log.starts_with("inkemu: flash at start: sha256 ")
+                && log.contains("\nerror: cannot write the flash file ")
+                && log.contains("what this run wrote to the flash is lost"),
+            "{log}"
+        );
+    }
+
+    #[test]
+    fn a_log_that_cannot_be_written_does_not_keep_the_flash_from_being_saved() {
+        struct Failing;
+        impl Write for Failing {
+            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
+                Err(std::io::Error::other("disk full"))
+            }
+            fn flush(&mut self) -> std::io::Result<()> {
+                Ok(())
+            }
+        }
+        let dir = tempfile::tempdir().unwrap();
+        let path = dir.path().join("x4.bin");
+        let (flash, content) = RunFlash::begin(Some(&path), false, None, MIB).unwrap();
+        let machine = ran(content, &writing(0x00));
+        assert!(flash.finish(&machine, &mut Failing));
+        assert_eq!(std::fs::read(&path).unwrap()[0x3000], 0x00);
+    }
 }
````

````diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -48,6 +48,18 @@ mod tests {
         }
     }
 
+    #[test]
+    fn a_run_whose_flash_file_could_not_be_written_exits_2_unless_strict_mmio_stopped_it() {
+        for end in RunEnd::ALL {
+            let expected = if end == RunEnd::StrictMmio {
+                EXIT_STRICT_MMIO
+            } else {
+                EXIT_ERROR
+            };
+            assert_eq!(end.exit_code_without_its_flash(), expected, "{end:?}");
+        }
+    }
+
     #[test]
     fn every_end_of_a_run_is_listed_once() {
         // The `match` has no wildcard: an end added to the enum does not compile here until
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p emu-cli --lib > /tmp/t9-red.log 2>&1; echo "exit=$?"; grep -m4 "^error" /tmp/t9-red.log
```

Expected: `exit=101`; the crate does not compile: ``cannot find type `RunFlash` in this scope``, ``no method named `exit_code_without_its_flash` found for enum `run::RunEnd` ``, ``no variant named `BothGiven` found for enum `flash_state::FlashStateError` ``.

- [ ] **Step 3: `RunFlash`**

````diff
--- a/crates/emu-cli/src/flash_state.rs
+++ b/crates/emu-cli/src/flash_state.rs
@@ -8,6 +8,7 @@
 //! over into a file.
 use crate::term::{hex, sanitize};
 use sha2::{Digest, Sha256};
+use soc_esp32c3::Esp32C3;
 use soc_esp32c3::flash::ERASED;
 use std::ffi::OsString;
 use std::fs::{File, OpenOptions, TryLockError};
@@ -61,6 +62,10 @@ pub enum FlashStateError {
         size: u32,
         found: String,
     },
+    #[error(
+        "both --flash and the flash file {path}, which exists, were given: the run cannot start from two flashes; drop --flash to go on from the flash file, or add --reset-flash to start again from the image and replace the file"
+    )]
+    BothGiven { path: String },
     #[error(
         "cannot write the flash file {path}: cannot {action} {target}: {detail}; what this run wrote to the flash is lost; check that the directory still exists, is writable and has {size} bytes free, then run again"
     )]
@@ -298,6 +303,136 @@ impl FlashState {
     }
 }
 
+/// The flash of one run of `inkemu run`: what it starts from, and what becomes of it when the
+/// run ends.
+///
+/// The content a run starts from is an input of the run, as the ROM and the device profile are:
+/// its SHA-256 is said in the summary. Writing the flash file is not part of the run: it happens
+/// after the last instruction, and enters no trace.
+#[derive(Debug)]
+pub struct RunFlash {
+    /// The flash file, locked, when the run was given one.
+    state: Option<FlashState>,
+    /// Whether the run starts from the content the flash file holds: the file then only has to
+    /// be written if the run modified the flash.
+    from_file: bool,
+    start_sha256: String,
+    size: u32,
+}
+
+impl RunFlash {
+    /// Decides what a run starts from and takes its flash file, before any machine exists.
+    ///
+    /// - No flash file: the run starts from `seed` (the `--flash` image) or from an erased
+    ///   flash, and nothing is kept when it ends.
+    /// - A flash file that exists: the run goes on from it. `seed` is then refused, since the
+    ///   run cannot start from both, unless `reset` asks to start again from the seed (or from
+    ///   an erased flash) and to replace the file.
+    /// - A flash file that does not exist yet: the run starts from `seed` or from an erased
+    ///   flash, and the file is created when it ends.
+    ///
+    /// Returns the content to build the machine from; `None` is an erased flash.
+    ///
+    /// # Errors
+    /// [`FlashStateError`]: the flash file cannot be used (see [`FlashState::open`],
+    /// [`FlashState::exists`], [`FlashState::load`]), or it exists and `seed` was given without
+    /// `reset`.
+    pub fn begin(
+        flash_file: Option<&Path>,
+        reset: bool,
+        seed: Option<Vec<u8>>,
+        size: u32,
+    ) -> Result<(RunFlash, Option<Vec<u8>>), FlashStateError> {
+        let state = flash_file
+            .map(|path| FlashState::open(path, size))
+            .transpose()?;
+        // An existing file is judged even when it is about to be replaced: a file of another
+        // size is not a flash of this device, and `--reset-flash` must not destroy it.
+        let from_file = match &state {
+            Some(state) if state.exists()? => {
+                if !reset && seed.is_some() {
+                    return Err(FlashStateError::BothGiven {
+                        path: state.shown.clone(),
+                    });
+                }
+                !reset
+            }
+            _ => false,
+        };
+        let content = match &state {
+            Some(state) if from_file => Some(state.load()?),
+            _ => seed,
+        };
+        let start_sha256 = content_sha256(content.as_deref().unwrap_or_default(), size);
+        Ok((
+            RunFlash {
+                state,
+                from_file,
+                start_sha256,
+                size,
+            },
+            content,
+        ))
+    }
+
+    /// The flash file of the run, when it has one.
+    #[must_use]
+    pub const fn state(&self) -> Option<&FlashState> {
+        self.state.as_ref()
+    }
+
+    /// Whether the run starts from the content of its flash file.
+    #[must_use]
+    pub const fn starts_from_the_file(&self) -> bool {
+        self.from_file
+    }
+
+    /// Ends the run for its flash: says in `log` what it started from and what became of it,
+    /// and writes the flash file when there is one and it does not already hold the content
+    /// (it did not exist, it is being replaced, or the run modified the flash).
+    ///
+    /// Returns `false` when the flash file had to be written and could not be: the error is in
+    /// `log`, and what the run wrote to the flash is lost. A failing log writer does not keep
+    /// the file from being written, nor change the answer.
+    pub fn finish(&self, machine: &Esp32C3, log: &mut dyn Write) -> bool {
+        let modified = machine.flash_modified_sectors();
+        let _ = writeln!(log, "inkemu: flash at start: sha256 {}", self.start_sha256);
+        let Some(state) = &self.state else {
+            if modified > 0 {
+                let _ = writeln!(
+                    log,
+                    "inkemu: flash at end: {modified} of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next"
+                );
+            }
+            return true;
+        };
+        if self.from_file && modified == 0 {
+            let _ = writeln!(
+                log,
+                "inkemu: flash at end: not modified; {} is left as it is",
+                state.shown()
+            );
+            return true;
+        }
+        let content = machine.flash_contents();
+        match state.save(&content) {
+            Ok(()) => {
+                let _ = writeln!(
+                    log,
+                    "inkemu: flash at end: sha256 {}; {modified} of its 4 KiB sectors were modified; saved to {}",
+                    content_sha256(&content, self.size),
+                    state.shown()
+                );
+                true
+            }
+            Err(e) => {
+                let _ = writeln!(log, "error: {e}");
+                false
+            }
+        }
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use super::*;
````

- [ ] **Step 4: The exit code of a run that lost its flash**

````diff
--- a/crates/emu-cli/src/run.rs
+++ b/crates/emu-cli/src/run.rs
@@ -170,6 +170,25 @@ impl RunEnd {
             RunEnd::Idle => EXIT_IDLE,
         }
     }
+
+    /// The process exit code of a run that ended this way and whose flash file could then not be
+    /// written: what the firmware wrote to the flash is lost, which is an error of the
+    /// environment (2) whatever ended the run, a wait nothing can end included. One end keeps
+    /// its own code, for the reason [`execute`] gives for the console: `--strict-mmio` exists
+    /// for CI, and its exit code must not depend on the state of a directory. The message of
+    /// the failed write is printed in every case.
+    #[must_use]
+    pub const fn exit_code_without_its_flash(self) -> u8 {
+        match self {
+            RunEnd::StrictMmio => EXIT_STRICT_MMIO,
+            RunEnd::Budget
+            | RunEnd::TimeLimit
+            | RunEnd::ConsoleClosed
+            | RunEnd::ConsoleError
+            | RunEnd::Idle
+            | RunEnd::Interrupted => EXIT_ERROR,
+        }
+    }
 }
 
 /// Writes `bytes` to `out` and flushes; `Ok(false)` when the reader is gone (a broken pipe).
````

- [ ] **Step 5: Run the suite**

```bash
cargo test --workspace > /tmp/t9.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t9.log
```

Expected: `exit=0` and `921`.

- [ ] **Step 6: Mutations**

Each row: apply the change, run `cargo test -p emu-cli --lib`, see the test fail, undo. Rows without a file are in `flash_state.rs`.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `if !reset && seed.is_some() {` → `if false && seed.is_some() {` | `a_seed_and_an_existing_flash_file_are_refused_unless_the_file_is_reset` |
| 2 | in `begin`, the value `!reset` of `from_file` → `true` | the same test |
| 3 | `if self.from_file && modified == 0 {` → `if modified == 0 {` | the same test (a reset that modified nothing must replace the file) |
| 4 | `if self.from_file && modified == 0 {` → `if self.from_file {` | `a_first_run_creates_the_flash_file_and_the_next_one_goes_on_from_it` |
| 5 | after `writeln!(log, "error: {e}")`, `false` → `true` | `a_flash_file_that_cannot_be_written_is_said_in_the_log_and_answered_by_no` |
| 6 | `Some(state) if state.exists()? => {` → `Some(state) if !reset && state.exists()? => {` | `a_reset_does_not_replace_a_file_that_is_not_a_flash_of_the_device` |
| 7 | `run.rs`: `RunEnd::Idle` moved out of the `EXIT_ERROR` arm to `RunEnd::Idle => EXIT_IDLE,` | `exit::tests::a_run_whose_flash_file_could_not_be_written_exits_2_unless_strict_mmio_stopped_it` |
| 8 | `content_sha256(content.as_deref().unwrap_or_default(), size)` → `hex(&Sha256::digest(content.as_deref().unwrap_or_default()))` | `without_a_flash_file_the_run_starts_from_the_seed_and_says_what_it_does_not_keep` |
| 9 | `if modified > 0 {` → `if modified < u32::MAX {` | the same test |
| 10 | `Some(state) if from_file => Some(state.load()?),` → `Some(_) if from_file => seed,` | `a_first_run_creates_the_flash_file_and_the_next_one_goes_on_from_it` |
| 11 | remove the `writeln!` of `inkemu: flash at start: sha256 …` | the same test, and two others |
| 12 | in the "saved" line, `content_sha256(&content, self.size)` → `self.start_sha256` | `a_first_run_creates_the_flash_file_and_the_next_one_goes_on_from_it` |

- [ ] **Step 7: Format, lint, commit**

```bash
/usr/bin/git commit -m "feat(cli): what a run starts from and what becomes of its flash when it ends

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-cli/src
```

---
## Task 10: `inkemu run --flash-file` keeps the flash of a device between runs

The two options, wired to `RunFlash`, with the tests through the binary and the ADR that says what an input of a run is. From this task on a user can keep what a firmware stores.

**Files:**
- Modify: `crates/emu-cli/src/main.rs` (`--flash-file`, `--reset-flash`, the header line, the summary lines, the exit code)
- Modify: `crates/emu-cli/src/exit.rs` (the help of exit code 2)
- Modify: `crates/emu-cli/tests/run_cli.rs` (6 tests that need no ROM, 6 ignored tests on the real ROM)
- Create: `docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md` (French)
- Modify: `.github/workflows/ci.yml` (the count of the ignored CLI tests: 10 → 16)

**Interfaces:**
- Consumes: `RunFlash::{begin, starts_from_the_file, finish}`, `RunEnd::exit_code_without_its_flash` (Task 9); `flash_file::load`, `DeviceProfile::flash.size()`, `instantiate_with_flash`.
- Produces, for the user:
  - `--flash-file <FILE>`: keep the flash of the device in this file from one run to the next.
  - `--reset-flash` (`requires = "flash_file"`: without `--flash-file` it is a usage error, exit 2): start from the `--flash` image, or from an erased flash, although the file exists, and replace it.
  - The header line of a run: `flash: x4.bin (flash file, 16 MiB)` when the run goes on from the file; `flash: <image> (<n> bytes in 16 MiB), kept in x4.bin` or `flash: erased (16 MiB), kept in x4.bin` when it starts fresh with a file; unchanged without a file.
  - After the summary of the run and the trace hash, the lines of `RunFlash::finish` (Task 9).
  - Exit code 2 when the flash file is refused before boot, and when it cannot be written at the end (3 after a `--strict-mmio` stop).
  - The flash file is taken (and locked) before the ROM is loaded: a refusal starts no download.

Nothing the firmware wrote sizes anything here. The path the user gave is echoed with its control characters escaped (`file_name`, `FlashState::shown`).

**The tests on the real ROM use two hand-written second stages**, built into a flash image by the helpers `run_cli.rs` already has: `counting_its_boots()` keeps in the flash the number of times it booted (the byte at flash offset 0x20000: each boot reads it through the data window, erases its sector, programs the next value and prints `boot N`), and `ready_then_busy()` prints `ready` and spins until something from outside ends the run. Both begin with `wait_for_uart0_to_drain()`: the ROM has just filled the transmit FIFO with its last lines, and a byte written to a full FIFO is lost, on the device as here.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -1093,3 +1093,599 @@ fn x4_boots_the_reference_firmware_and_a_device_time_limit_ends_its_countdown_as
     );
     assert!(!log.contains("warning:"), "{log}");
 }
+
+/// The flash of the X4, the device every test here runs.
+const X4_FLASH_BYTES: u64 = 16 << 20;
+
+/// `inkemu run --device x4 --flash-file <file> <more>` with a ROM directory that holds nothing:
+/// a refusal of the flash file must come before any attempt to load a ROM.
+fn run_with_flash_file(file: &std::path::Path, more: &[&str]) -> assert_cmd::assert::Assert {
+    let empty = tempfile::tempdir().unwrap();
+    inkemu()
+        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
+        .arg(empty.path())
+        .arg("--flash-file")
+        .arg(file)
+        .args(more)
+        .assert()
+}
+
+/// A file of `len` zero bytes, sparse: nothing that large is written to the disk.
+fn sparse_file(path: &std::path::Path, len: u64) {
+    std::fs::File::create(path).unwrap().set_len(len).unwrap();
+}
+
+#[test]
+fn run_help_documents_the_flash_file_its_lock_and_its_reset() {
+    inkemu().args(["run", "--help"]).assert().code(0).stdout(
+        contains("--flash-file <FILE>")
+            .and(contains("FILE.lock"))
+            .and(contains("--reset-flash"))
+            .and(contains("The image is only read"))
+            .and(contains(
+                "the flash file could not be written when the run ended",
+            )),
+    );
+}
+
+#[test]
+fn a_reset_of_the_flash_without_a_flash_file_is_a_usage_error() {
+    // An empty ROM directory and a limit: were the option accepted, the command would end on the
+    // ROM it does not have, and never download one or run for ever.
+    let empty = tempfile::tempdir().unwrap();
+    inkemu()
+        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
+        .arg(empty.path())
+        .arg("--reset-flash")
+        .timeout(std::time::Duration::from_secs(60))
+        .assert()
+        .code(2)
+        .stderr(
+            contains("--flash-file <FILE>")
+                .and(contains("required arguments were not provided"))
+                .and(contains("esp32c3_rev3_rom.elf").not()),
+        );
+}
+
+#[test]
+fn a_flash_file_of_another_size_is_refused_before_boot_with_both_sizes_and_left_as_it_is() {
+    let dir = tempfile::tempdir().unwrap();
+    let file = dir.path().join("x4.bin");
+    // A firmware image given to the wrong option: with or without a reset, it must survive.
+    let image = build_flash_image(&ImageSpec::default());
+    std::fs::write(&file, &image).unwrap();
+    for more in [&[][..], &["--reset-flash"][..]] {
+        run_with_flash_file(&file, more).code(2).stderr(
+            contains("x4.bin")
+                .and(contains(format!("is {} bytes", image.len())))
+                .and(contains("16777216 bytes"))
+                .and(contains("never padded or cut"))
+                .and(contains("; use it with the device it was written for"))
+                .and(contains("esp32c3_rev3_rom.elf").not()),
+        );
+        assert_eq!(std::fs::read(&file).unwrap(), image);
+    }
+}
+
+#[test]
+fn an_image_together_with_a_flash_file_that_exists_is_refused_before_boot() {
+    let dir = tempfile::tempdir().unwrap();
+    let file = dir.path().join("x4.bin");
+    sparse_file(&file, X4_FLASH_BYTES);
+    let image = dir.path().join("flash.bin");
+    std::fs::write(&image, build_flash_image(&ImageSpec::default())).unwrap();
+    run_with_flash_file(&file, &["--flash", image.to_str().unwrap()])
+        .code(2)
+        .stderr(
+            contains("both --flash and the flash file")
+                .and(contains("x4.bin"))
+                .and(contains("; drop --flash to go on from the flash file"))
+                .and(contains("--reset-flash"))
+                .and(contains("esp32c3_rev3_rom.elf").not()),
+        );
+    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
+}
+
+#[test]
+fn a_flash_file_that_another_run_holds_is_refused_before_boot() {
+    let dir = tempfile::tempdir().unwrap();
+    let file = dir.path().join("x4.bin");
+    // This process is the other run: it holds the lock a run of inkemu takes.
+    let held = emu_cli::flash_state::FlashState::open(&file, 16 << 20).unwrap();
+    run_with_flash_file(&file, &[]).code(2).stderr(
+        contains("is in use by another inkemu run")
+            .and(contains("x4.bin.lock"))
+            .and(contains("; wait for that run to end"))
+            .and(contains("esp32c3_rev3_rom.elf").not()),
+    );
+    drop(held);
+    // The lock is free again: the same command now gets as far as the ROM it does not have.
+    run_with_flash_file(&file, &[])
+        .code(2)
+        .stderr(contains("esp32c3_rev3_rom.elf").and(contains("in use").not()));
+    assert!(!file.exists(), "a run that never started writes nothing");
+}
+
+#[test]
+fn a_flash_file_in_a_directory_that_does_not_exist_is_refused_before_boot() {
+    let dir = tempfile::tempdir().unwrap();
+    let file = dir.path().join("missing").join("x4.bin");
+    run_with_flash_file(&file, &[]).code(2).stderr(
+        contains("cannot create the lock file")
+            .and(contains(
+                "; check that the directory exists and is writable",
+            ))
+            .and(contains("esp32c3_rev3_rom.elf").not()),
+    );
+}
+
+/// Code that waits until the transmit FIFO of UART0 is empty. A second stage that prints must
+/// start with it: the ROM has just filled the FIFO with its last lines, and a byte written to a
+/// full FIFO is lost, on the device as here. `UART.STATUS` [25:16] is `TXFIFO_CNT`: the upper
+/// half of the register, modulo 1024.
+fn wait_for_uart0_to_drain() -> [u32; 5] {
+    use cpu_rv32::asm::{addi, bne, lhu, lui, remu};
+    use soc_esp32c3::test_support::{T0, T1};
+    const T2: u8 = 7;
+    let status = svd_regs::esp32c3::addr("UART0", "STATUS");
+    let page = status & 0xFFFF_F000;
+    [
+        lui(T0, page),
+        addi(T2, 0, 1024),
+        lhu(T1, T0, (status - page + 2).cast_signed()),
+        remu(T1, T1, T2),
+        bne(T1, 0, -8),
+    ]
+}
+
+/// A second stage that keeps in the flash the number of times it booted, and prints it: `boot 1`
+/// on an erased flash, `boot 2` on the flash that boot left, and so on. The count is the byte at
+/// flash offset 0x20000, past everything the images built here hold; each boot erases its sector
+/// and programs the next value. The byte is read through the data window (MMU entry 0), once,
+/// before it is written.
+fn counting_its_boots() -> Vec<u32> {
+    use cpu_rv32::asm::{addi, lbu, lui, sb, sw};
+    use soc_esp32c3::test_support::{T0, flash_erase_sector, print_program};
+    const A0: u8 = 10;
+    const A1: u8 = 11;
+    const S0: u8 = 8;
+    const COUNT_AT: u32 = 0x2_0000;
+    let spi1 = |register| svd_regs::esp32c3::addr("SPI1", register);
+    let mut program = stop_watchdogs();
+    program.extend(store_register(
+        soc_esp32c3::periph::mmu::MMU_TABLE_BASE,
+        COUNT_AT >> 16,
+    ));
+    program.extend([
+        lui(S0, soc_esp32c3::memmap::FLASH_DBUS_BASE),
+        lbu(A0, S0, 0), // the count the flash holds: 0xFF when erased
+        addi(A0, A0, 1),
+        // Through a byte of RAM, to keep the low byte: 0xFF + 1 is the count 0.
+        lui(S0, soc_esp32c3::memmap::SRAM_DBUS_BASE),
+        sb(A0, S0, 0),
+        lbu(A0, S0, 0),
+    ]);
+    program.extend(wait_for_uart0_to_drain());
+    program.extend(print_program(
+        svd_regs::esp32c3::addr("UART0", "FIFO"),
+        b"boot ",
+    ));
+    // `T0` still holds the address of the FIFO: the digit, then the end of the line.
+    program.extend([
+        addi(A1, A0, i32::from(b'1')),
+        sw(A1, T0, 0),
+        addi(A1, 0, i32::from(b'\n')),
+        sw(A1, T0, 0),
+    ]);
+    program.extend(flash_erase_sector(COUNT_AT));
+    // Write enable, the count in `W0`, one byte of it programmed at `COUNT_AT`.
+    program.extend(store_register(spi1("CMD"), 1 << 30));
+    let w0 = spi1("W0");
+    let page = w0.wrapping_add(0x800) & 0xFFFF_F000;
+    program.extend([
+        lui(T0, page),
+        sw(A0, T0, w0.wrapping_sub(page).cast_signed()),
+    ]);
+    program.extend(store_register(spi1("ADDR"), (1 << 24) | COUNT_AT));
+    program.extend(store_register(spi1("CMD"), 1 << 25));
+    program.push(HANG);
+    program
+}
+
+/// What a run of [`counting_its_boots`] left: its exit code, its console and its log.
+struct Counted {
+    code: Option<i32>,
+    console: String,
+    log: String,
+}
+
+impl Counted {
+    /// The hexadecimal digest of the summary line that starts with `prefix`.
+    fn sha256(&self, prefix: &str) -> String {
+        self.log
+            .lines()
+            .find_map(|line| line.strip_prefix(prefix))
+            .unwrap_or_else(|| panic!("no line `{prefix}` in:\n{}", self.log))
+            .chars()
+            .take_while(char::is_ascii_hexdigit)
+            .collect()
+    }
+}
+
+const FLASH_AT_START: &str = "inkemu: flash at start: sha256 ";
+const FLASH_AT_END: &str = "inkemu: flash at end: sha256 ";
+
+/// Runs `inkemu run --device x4 --strict-mmio --trace-hash` with the real ROM and `more`, for the
+/// cycles the ROM and [`counting_its_boots`] need.
+fn count_a_boot(more: &[&std::ffi::OsStr]) -> Counted {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let output = inkemu()
+        .args(["run", "--device", "x4", "--strict-mmio", "--trace-hash"])
+        .args(["--max-cycles", "5000000", "--rom-dir", &rom_dir])
+        .args(more)
+        .timeout(std::time::Duration::from_secs(60))
+        .output()
+        .unwrap();
+    Counted {
+        code: output.status.code(),
+        console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
+        log: String::from_utf8_lossy(&output.stderr).into_owned(),
+    }
+}
+
+fn sha256_of(path: &std::path::Path) -> String {
+    use sha2::Digest as _;
+    emu_cli::term::hex(&sha2::Sha256::digest(std::fs::read(path).unwrap()))
+}
+
+/// Persistence through the real command: the flash file is created by the first run, the second
+/// run goes on from it, an image is refused next to it, and a reset starts again.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next() {
+    let (dir, image) = flash_image_of(&counting_its_boots());
+    let image_bytes = std::fs::read(&image).unwrap();
+    let file = dir.path().join("x4.bin");
+    let (flash, flash_file) = (
+        std::ffi::OsStr::new("--flash"),
+        std::ffi::OsStr::new("--flash-file"),
+    );
+
+    let first = count_a_boot(&[flash, image.as_os_str(), flash_file, file.as_os_str()]);
+    assert_eq!(first.code, Some(0), "{}", first.log);
+    assert!(
+        first.console.ends_with("entry 0x40380000\nboot 1\n"),
+        "{}",
+        first.console
+    );
+    assert!(
+        first.log.contains("flash: flash.bin (") && first.log.contains("), kept in x4.bin\n"),
+        "{}",
+        first.log
+    );
+    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
+    assert_eq!(first.sha256(FLASH_AT_END), sha256_of(&file));
+    assert!(
+        first
+            .log
+            .contains("; 1 of its 4 KiB sectors were modified; saved to "),
+        "{}",
+        first.log
+    );
+    assert!(!first.log.contains("warning:"), "{}", first.log);
+    assert_eq!(
+        std::fs::read(&image).unwrap(),
+        image_bytes,
+        "--flash is only read"
+    );
+
+    // The second run is given the file alone: a power-on from the flash the first one left.
+    let second = count_a_boot(&[flash_file, file.as_os_str()]);
+    assert_eq!(second.code, Some(0), "{}", second.log);
+    assert!(
+        second.console.ends_with("entry 0x40380000\nboot 2\n"),
+        "{}",
+        second.console
+    );
+    assert!(
+        second.log.contains("flash: x4.bin (flash file, 16 MiB)\n"),
+        "{}",
+        second.log
+    );
+    assert_eq!(
+        second.sha256(FLASH_AT_START),
+        first.sha256(FLASH_AT_END),
+        "what the second run starts from is what the first one saved"
+    );
+    assert_eq!(second.sha256(FLASH_AT_END), sha256_of(&file));
+    assert_ne!(second.sha256(FLASH_AT_END), second.sha256(FLASH_AT_START));
+    let after_second = std::fs::read(&file).unwrap();
+    assert_eq!(after_second[0x2_0000], 1, "the count of two boots, from 0");
+
+    // The image next to the file that exists: refused, and nothing is written.
+    let refused = count_a_boot(&[flash, image.as_os_str(), flash_file, file.as_os_str()]);
+    assert_eq!(refused.code, Some(2), "{}", refused.log);
+    assert!(
+        refused.log.contains("both --flash and the flash file"),
+        "{}",
+        refused.log
+    );
+    assert!(
+        refused.console.is_empty(),
+        "nothing ran: {}",
+        refused.console
+    );
+    assert_eq!(sha256_of(&file), second.sha256(FLASH_AT_END));
+
+    // A reset starts again from the image and replaces the file.
+    let reset = count_a_boot(&[
+        flash,
+        image.as_os_str(),
+        flash_file,
+        file.as_os_str(),
+        std::ffi::OsStr::new("--reset-flash"),
+    ]);
+    assert_eq!(reset.code, Some(0), "{}", reset.log);
+    assert!(reset.console.ends_with("boot 1\n"), "{}", reset.console);
+    assert_eq!(reset.sha256(FLASH_AT_END), first.sha256(FLASH_AT_END));
+    assert_eq!(sha256_of(&file), first.sha256(FLASH_AT_END));
+    assert_eq!(
+        std::fs::read(&image).unwrap(),
+        image_bytes,
+        "--flash is only read"
+    );
+    let mut names: Vec<_> = std::fs::read_dir(dir.path())
+        .unwrap()
+        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
+        .collect();
+    names.sort();
+    assert_eq!(
+        names,
+        ["flash.bin", "x4.bin", "x4.bin.lock"],
+        "and nothing else is left"
+    );
+}
+
+/// Without a flash file nothing is kept, the summary says how much, and the image is not written.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_run_without_a_flash_file_says_what_it_does_not_keep_and_leaves_the_image_as_it_is() {
+    let (dir, image) = flash_image_of(&counting_its_boots());
+    let image_bytes = std::fs::read(&image).unwrap();
+    let flash = std::ffi::OsStr::new("--flash");
+    for _ in 0..2 {
+        let run = count_a_boot(&[flash, image.as_os_str()]);
+        assert_eq!(run.code, Some(0), "{}", run.log);
+        assert!(
+            run.console.ends_with("boot 1\n"),
+            "every run is the first: {}",
+            run.console
+        );
+        assert!(
+            run.log.contains(
+                "inkemu: flash at end: 1 of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next\n"
+            ),
+            "{}",
+            run.log
+        );
+        assert!(!run.log.contains(FLASH_AT_END), "{}", run.log);
+        assert_eq!(std::fs::read(&image).unwrap(), image_bytes);
+    }
+    assert_eq!(
+        std::fs::read_dir(dir.path()).unwrap().count(),
+        1,
+        "the image alone"
+    );
+}
+
+/// Determinism with a flash file: the content a run starts from is an input, and nothing else
+/// about the file is. Two runs from the same content have the same trace and leave the same file.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn the_same_starting_flash_file_gives_the_same_trace_and_the_same_final_file() {
+    let (dir, image) = flash_image_of(&counting_its_boots());
+    let (flash, flash_file) = (
+        std::ffi::OsStr::new("--flash"),
+        std::ffi::OsStr::new("--flash-file"),
+    );
+    // A flash file to start from: the one a first run leaves.
+    let origin = dir.path().join("origin.bin");
+    let made = count_a_boot(&[flash, image.as_os_str(), flash_file, origin.as_os_str()]);
+    assert_eq!(made.code, Some(0), "{}", made.log);
+    let run_from_a_copy = |name: &str| {
+        let file = dir.path().join(name);
+        std::fs::copy(&origin, &file).unwrap();
+        let run = count_a_boot(&[flash_file, file.as_os_str()]);
+        assert_eq!(run.code, Some(0), "{}", run.log);
+        assert!(run.console.ends_with("boot 2\n"), "{}", run.console);
+        (run, sha256_of(&file))
+    };
+    let (one, file_one) = run_from_a_copy("one.bin");
+    let (two, file_two) = run_from_a_copy("two.bin");
+    assert!(trace_hash_line(&one.log).is_some(), "{}", one.log);
+    assert_eq!(trace_hash_line(&one.log), trace_hash_line(&two.log));
+    assert_eq!(one.console, two.console);
+    assert_eq!(file_one, file_two, "the same final flash file");
+    assert_eq!(one.sha256(FLASH_AT_END), file_one);
+    assert_eq!(one.sha256(FLASH_AT_START), sha256_of(&origin));
+    // The first run of the image, with and without a flash file, is the same run: keeping the
+    // flash is no part of the trace.
+    let without = count_a_boot(&[flash, image.as_os_str()]);
+    assert_eq!(trace_hash_line(&without.log), trace_hash_line(&made.log));
+    assert_eq!(without.sha256(FLASH_AT_START), made.sha256(FLASH_AT_START));
+    // The trace hash is the path the CPU took, and this firmware takes the same path whatever
+    // count it reads: the second boot has the trace hash of the first. What tells the two runs
+    // apart is the flash each started from, which is why the summary says it.
+    assert_eq!(trace_hash_line(&without.log), trace_hash_line(&one.log));
+    assert_ne!(without.sha256(FLASH_AT_START), one.sha256(FLASH_AT_START));
+    assert_ne!(without.console, one.console);
+}
+
+/// A second stage that stops the watchdogs, prints `ready` and spins: a run that goes on until
+/// something from outside ends it.
+#[cfg(unix)]
+fn ready_then_busy() -> Vec<u32> {
+    let mut program = stop_watchdogs();
+    program.extend(wait_for_uart0_to_drain());
+    program.extend(soc_esp32c3::test_support::print_program(
+        svd_regs::esp32c3::addr("UART0", "FIFO"),
+        b"ready\n",
+    ));
+    program.push(HANG);
+    program
+}
+
+/// [`run_of`] with a flash file.
+#[cfg(unix)]
+fn run_with_file(
+    rom_dir: &str,
+    image: &std::path::Path,
+    file: &std::path::Path,
+) -> std::process::Command {
+    let mut command = run_of(rom_dir, image);
+    command.arg("--flash-file").arg(file);
+    command
+}
+
+/// Two processes on one flash file: the second is refused while the first runs, and the first
+/// Ctrl-C ends the first with its flash file written.
+#[cfg(unix)]
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_second_run_on_a_flash_file_is_refused_while_the_first_goes_on_and_ctrl_c_saves_it() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let (dir, image) = flash_image_of(&ready_then_busy());
+    let file = dir.path().join("x4.bin");
+    let limit = std::time::Duration::from_secs(60);
+    let first = live::start(run_with_file(&rom_dir, &image, &file), |_| false);
+    first.wait_for_line("ready", limit);
+    assert!(
+        !file.exists(),
+        "the flash file is not written while the run goes on"
+    );
+
+    // A limit: were the second run let in, it would end by itself, with exit code 0, instead of
+    // spinning beside the first for ever.
+    let second = run_with_file(&rom_dir, &image, &file)
+        .args(["--max-cycles", "3000000"])
+        .output()
+        .unwrap();
+    assert_eq!(second.status.code(), Some(2));
+    let refusal = String::from_utf8_lossy(&second.stderr).into_owned();
+    assert!(
+        refusal.contains("is in use by another inkemu run") && refusal.contains("x4.bin.lock"),
+        "{refusal}"
+    );
+    assert!(second.stdout.is_empty(), "the second run never started");
+    assert!(!file.exists(), "and it wrote nothing");
+
+    first.interrupt();
+    let (code, log) = first.finish(limit);
+    assert_eq!(code, Some(0), "{log}");
+    assert!(
+        log.contains("inkemu: interrupted by the user\n")
+            && log.contains("; 0 of its 4 KiB sectors were modified; saved to "),
+        "{log}"
+    );
+    assert_eq!(std::fs::metadata(&file).unwrap().len(), X4_FLASH_BYTES);
+    // The first run is over: its lock is free, and the file it left is taken as it is.
+    let third = live::start(
+        {
+            let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_inkemu"));
+            command
+                .args([
+                    "run",
+                    "--device",
+                    "x4",
+                    "--rom-dir",
+                    &rom_dir,
+                    "--flash-file",
+                ])
+                .arg(&file);
+            command
+        },
+        |_| false,
+    );
+    third.wait_for_line("ready", limit);
+    third.interrupt();
+    let (code, log) = third.finish(limit);
+    assert_eq!(code, Some(0), "{log}");
+    assert!(
+        log.contains("inkemu: flash at end: not modified; "),
+        "{log}"
+    );
+}
+
+/// The flash file cannot be written when the run ends, because its directory is gone: the run
+/// says so, says what is lost, and exits 2.
+#[cfg(unix)]
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_flash_file_whose_directory_vanished_during_the_run_exits_2_and_says_what_is_lost() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let (dir, image) = flash_image_of(&ready_then_busy());
+    let kept_in = dir.path().join("flash");
+    std::fs::create_dir(&kept_in).unwrap();
+    let limit = std::time::Duration::from_secs(60);
+    let run = live::start(
+        run_with_file(&rom_dir, &image, &kept_in.join("x4.bin")),
+        |_| false,
+    );
+    run.wait_for_line("ready", limit);
+    std::fs::remove_dir_all(&kept_in).unwrap();
+    run.interrupt();
+    let (code, log) = run.finish(limit);
+    assert_eq!(code, Some(2), "{log}");
+    assert!(
+        log.contains("\ninkemu: stopped at pc ")
+            && log.contains("\nerror: cannot write the flash file ")
+            && log.contains(
+                "what this run wrote to the flash is lost; check that the directory still exists"
+            ),
+        "the summary, then the error: {log}"
+    );
+}
+
+/// The same with a directory that became read-only: nothing can be created in it.
+#[cfg(unix)]
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR"]
+fn a_flash_file_whose_directory_became_read_only_exits_2_and_leaves_no_file() {
+    use std::os::unix::fs::PermissionsExt as _;
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let (dir, image) = flash_image_of(&ready_then_busy());
+    let kept_in = dir.path().join("flash");
+    std::fs::create_dir(&kept_in).unwrap();
+    let read_only = |yes: bool| {
+        let mode = if yes { 0o555 } else { 0o755 };
+        std::fs::set_permissions(&kept_in, std::fs::Permissions::from_mode(mode)).unwrap();
+    };
+    // The administrator writes where the mode says nobody can: the case cannot be made then.
+    read_only(true);
+    let probe = std::fs::File::create(kept_in.join("probe")).is_ok();
+    read_only(false);
+    assert!(
+        !probe,
+        "this test needs a user whom a read-only directory stops; run it as one"
+    );
+    let limit = std::time::Duration::from_secs(60);
+    let run = live::start(
+        run_with_file(&rom_dir, &image, &kept_in.join("x4.bin")),
+        |_| false,
+    );
+    run.wait_for_line("ready", limit);
+    read_only(true);
+    run.interrupt();
+    let (code, log) = run.finish(limit);
+    read_only(false);
+    assert_eq!(code, Some(2), "{log}");
+    assert!(
+        log.contains("\nerror: cannot write the flash file ") && log.contains("cannot create "),
+        "{log}"
+    );
+    let left: Vec<_> = std::fs::read_dir(&kept_in)
+        .unwrap()
+        .map(|entry| entry.unwrap().file_name())
+        .collect();
+    assert_eq!(left, ["x4.bin.lock"], "no flash file, no temporary file");
+}
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p emu-cli --test run_cli > /tmp/t10-red.log 2>&1; echo "exit=$?"; grep "^test .* FAILED$" /tmp/t10-red.log | sort
```

Expected: `exit=101`, 6 failed tests (25 pass, 18 ignored), each with `error: unexpected argument '--flash-file' found` or `'--reset-flash'`: `a_flash_file_in_a_directory_that_does_not_exist_is_refused_before_boot`, `a_flash_file_of_another_size_is_refused_before_boot_with_both_sizes_and_left_as_it_is`, `a_flash_file_that_another_run_holds_is_refused_before_boot`, `a_reset_of_the_flash_without_a_flash_file_is_a_usage_error`, `an_image_together_with_a_flash_file_that_exists_is_refused_before_boot`, `run_help_documents_the_flash_file_its_lock_and_its_reset`.

- [ ] **Step 3: The options**

````diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -4,6 +4,7 @@ use clap::{Parser, Subcommand};
 use cpu_rv32::{HartConfig, MisalignedPolicy};
 use emu_cli::exit::{EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
 use emu_cli::flash_file;
+use emu_cli::flash_state::RunFlash;
 use emu_cli::rom_store::{
     self, AnnouncingFetch, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource,
 };
@@ -59,9 +60,23 @@ enum Command {
         /// Full flash image to boot: a dump of a device (`esptool.py read_flash 0 ALL flash.bin`),
         /// or the output of `esptool.py merge_bin`. Its bootloader, partition table and
         /// application run as on the device. Without it the flash is erased and the ROM reports
-        /// an invalid header, as a blank chip does.
+        /// an invalid header, as a blank chip does. The image is only read: what the firmware
+        /// writes to the flash is kept by --flash-file, or lost when the run ends.
         #[arg(long)]
         flash: Option<PathBuf>,
+        /// Keep the flash of the device in this file from one run to the next: a raw image of
+        /// exactly the size of the flash. A run starts from the file when it exists (--flash is
+        /// then refused unless --reset-flash is given) and writes it when it ends: at a limit,
+        /// at the first Ctrl-C, at a --strict-mmio stop, when the CPU waits for ever. The file
+        /// is replaced in one step, never written while the run goes on, so a run that is
+        /// killed leaves the file of before. A lock file beside it (FILE.lock) refuses a second
+        /// run on the same file; it is left in place.
+        #[arg(long, value_name = "FILE")]
+        flash_file: Option<PathBuf>,
+        /// Start from the --flash image, or from an erased flash, although the flash file
+        /// exists, and replace the file when the run ends.
+        #[arg(long, requires = "flash_file")]
+        reset_flash: bool,
         /// Stop when the CPU has executed this many cycles of its clock. The limit counts executed
         /// cycles, not device time: a sleeping CPU executes none, so a firmware that sleeps
         /// reaches it late, and one whose sleep nothing interrupts never does; --max-time bounds
@@ -153,6 +168,8 @@ fn main() -> ExitCode {
         Command::Run {
             device,
             flash,
+            flash_file,
+            reset_flash,
             max_cycles,
             max_time,
             strict_mmio,
@@ -161,7 +178,11 @@ fn main() -> ExitCode {
             trace_hash,
         } => run_device(
             &device,
-            flash.as_deref(),
+            &FlashOptions {
+                image: flash.as_deref(),
+                file: flash_file.as_deref(),
+                reset: reset_flash,
+            },
             rom_dir,
             MachineOptions {
                 strict_mmio,
@@ -235,9 +256,28 @@ const fn image_chip(profile: &DeviceProfile) -> ChipId {
     }
 }
 
+/// What `inkemu run` was told about the flash of the device.
+struct FlashOptions<'a> {
+    /// `--flash`: the image a run starts from, never written.
+    image: Option<&'a Path>,
+    /// `--flash-file`: where the flash is kept from one run to the next.
+    file: Option<&'a Path>,
+    /// `--reset-flash`: start again from the image and replace the file.
+    reset: bool,
+}
+
+/// The last component of `path`, for a line that names a file the user gave: escaped, since it
+/// comes straight from the command line.
+fn file_name(path: &Path) -> String {
+    sanitize(&path.file_name().map_or_else(
+        || path.display().to_string(),
+        |name| name.to_string_lossy().into_owned(),
+    ))
+}
+
 fn run_device(
     device: &str,
-    flash: Option<&Path>,
+    flash: &FlashOptions<'_>,
     rom_dir: Option<PathBuf>,
     machine_options: MachineOptions,
     run_options: &RunOptions,
@@ -246,15 +286,40 @@ fn run_device(
         Ok(p) => p,
         Err(e) => return fail(&e),
     };
-    // The flash file is checked first: refusing it needs no ROM, and no download should start for a
-    // run that cannot happen.
-    let flash_image = match flash {
-        Some(path) => match flash_file::load(path, image_chip(&profile), profile.flash.size()) {
-            Ok(bytes) => Some((path, bytes)),
+    let flash_size = profile.flash.size();
+    // The flash image and the flash file are checked first: refusing them needs no ROM, and no
+    // download should start for a run that cannot happen.
+    let image = match flash.image {
+        Some(path) => match flash_file::load(path, image_chip(&profile), flash_size) {
+            Ok(bytes) => Some(bytes),
             Err(e) => return fail(&e),
         },
         None => None,
     };
+    // What the flash holds when nothing is kept from an earlier run.
+    let fresh_note = match (flash.image, &image) {
+        (Some(path), Some(bytes)) => format!(
+            "{} ({} bytes in {} MiB)",
+            file_name(path),
+            bytes.len(),
+            flash_size >> 20
+        ),
+        _ => format!("erased ({} MiB)", flash_size >> 20),
+    };
+    // From here on the flash file is locked, until the process ends.
+    let (run_flash, content) = match RunFlash::begin(flash.file, flash.reset, image, flash_size) {
+        Ok(begun) => begun,
+        Err(e) => return fail(&e),
+    };
+    let flash_note = match flash.file {
+        Some(path) if run_flash.starts_from_the_file() => format!(
+            "flash: {} (flash file, {} MiB)",
+            file_name(path),
+            flash_size >> 20
+        ),
+        Some(path) => format!("flash: {fresh_note}, kept in {}", file_name(path)),
+        None => format!("flash: {fresh_note}"),
+    };
     let entry = rom_store::rom_entry_for(profile.chip_revision);
     let http = HttpFetch;
     let mut stderr = io::stderr();
@@ -278,20 +343,8 @@ fn run_device(
         Ok(rom) => rom,
         Err(e) => return fail(&e),
     };
-    let flash_note = match &flash_image {
-        Some((path, bytes)) => format!(
-            "flash: {} ({} bytes in {} MiB)",
-            sanitize(&path.file_name().map_or_else(
-                || path.display().to_string(),
-                |name| name.to_string_lossy().into_owned()
-            )),
-            bytes.len(),
-            profile.flash.size() >> 20
-        ),
-        None => format!("flash: erased ({} MiB)", profile.flash.size() >> 20),
-    };
-    let mut machine = match flash_image {
-        Some((_, bytes)) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
+    let mut machine = match content {
+        Some(bytes) => match profile.instantiate_with_flash(rom, bytes, machine_options) {
             Ok(machine) => machine,
             Err(e) => return fail(&e),
         },
@@ -318,7 +371,14 @@ fn run_device(
         &mut io::stdout().lock(),
         &mut io::stderr().lock(),
     );
-    ExitCode::from(end.exit_code())
+    // The run is over: what follows is no part of it, and of no trace. A second Ctrl-C from
+    // here on still ends the process at once, and the flash file is then the one of before.
+    let code = if run_flash.finish(&machine, &mut io::stderr().lock()) {
+        end.exit_code()
+    } else {
+        end.exit_code_without_its_flash()
+    };
+    ExitCode::from(code)
 }
 
 /// Whether whoever was reading standard output went away (`inkemu run | head`, once `head` has
````

````diff
--- a/crates/emu-cli/src/exit.rs
+++ b/crates/emu-cli/src/exit.rs
@@ -20,7 +20,9 @@ pub const RUN_EXIT_CODES_HELP: &str = "\
 Exit codes:
   0  the run ended normally: the cycle limit or the device-time limit was reached, Ctrl-C was
      pressed, or the reader of standard output went away
-  2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
+  2  error: unknown device, missing or invalid ROM, refused flash image or flash file, standard
+     output cannot be written, or the flash file could not be written when the run ended (what
+     the firmware wrote to the flash is then lost)
   3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
      emulator cannot perform faithfully, flash that is not mapped, or a read through a flash
      window that may differ from the device)
````

- [ ] **Step 4: Run the suite and the ignored tests; update the count of the CI**

```bash
cargo test --workspace > /tmp/t10.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t10.log
tools/ci/run-ignored-tests.sh 16 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware > /tmp/t10-i2.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` and `927`; `exit=0`. Run the ignored tests as a user whom a read-only directory stops (not root): `a_flash_file_whose_directory_became_read_only_exits_2_and_leaves_no_file` says so otherwise.

````diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -91,7 +91,7 @@ jobs:
         # un-ignored test would otherwise leave this job green with nothing proven.
         run: |
           tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored
-          tools/ci/run-ignored-tests.sh 10 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware
+          tools/ci/run-ignored-tests.sh 16 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware
   reference-boot:
     # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM
     # and runs it through a whole life: the application, its countdown, the restart it asks for
````

- [ ] **Step 5: ADR 0004**

The starting flash content is an input of a run; the ADR says what that means, the life of the file, and what the recording of SP1b-4 must hold. Its point on the trace hash was found while writing the tests of this task: the hash is the path of the CPU, not its data, so the first and the second boot of `counting_its_boots()` have the same trace hash although they start from two flash contents. `the_same_starting_flash_file_gives_the_same_trace_and_the_same_final_file` shows it.

````markdown
# ADR 0004 — Entrées d'un run et fichier de flash

- Date : 2026-10-12
- Statut : accepté (plan SP1b-3b)

## Contexte

La flash s'écrit désormais (ADR 0002, révision du 2026-10-12). Ce qu'un firmware y écrit doit pouvoir survivre à la fin d'un run : la spec demande que chaque appareil virtuel possède son fichier de flash (section 5.1).
La même spec promet qu'une machine est déterministe (section 3.1) : deux runs identiques exécutent la même chose. Tant que la flash était en lecture seule, « identiques » allait de soi : même ROM, même image, mêmes options. Dès que la flash persiste, son contenu au départ d'un run n'est plus une constante, et un fichier que le run lit puis réécrit pourrait faire dépendre un run de l'hôte : du chemin, de l'instant, d'un autre processus.
Il faut donc dire ce qui est une entrée d'un run et ce qui n'en est pas, puis donner au fichier de flash une vie qui respecte cette frontière. Le modèle (`soc-esp32c3`) ne fait aucune entrée-sortie sur l'hôte : il se compile pour WebAssembly, et la CI le vérifie.

## Décision

1. **Les entrées d'un run sont au nombre de quatre.**
   - L'image de la ROM masquée, dont le SHA-256 est épinglé et vérifié à chaque chargement.
   - Le profil de l'appareil : révision de la puce, taille et identifiant de la flash, broches de démarrage.
   - Le contenu de la flash au départ, entier, octets effacés compris.
   - Les options qui changent ce que la machine fait ou l'endroit où le run est coupé : `--strict-mmio`, `--max-cycles`, `--max-time`.

   Rien d'autre n'entre dans un run : ni l'heure de l'hôte, ni le chemin ou l'existence du fichier de flash, ni son écriture, ni la console relayée (`--console` choisit ce qui est montré, pas ce qui est exécuté). Ctrl-C et le départ du lecteur de la sortie coupent le run à un endroit qui dépend de l'hôte : un tel run n'est pas reproductible, ce que disait déjà `RunEnd::Interrupted`.
   Les entrées que le firmware ne peut pas encore recevoir (octets reçus sur une console, boutons, carte SD) s'ajouteront à cette liste avec les plans qui les apportent (SP1b-3d et suivants), datées en temps d'appareil.
2. **Le contenu de la flash au départ est nommé par son SHA-256, que le résumé du run imprime.** Le hash de trace est le chemin du CPU (la suite des adresses exécutées), pas ses données : deux runs partis de deux contenus différents ont le même hash de trace quand le firmware suit le même chemin (un test le montre avec un compteur de démarrages). Le hash de trace et le SHA-256 du contenu de départ identifient un run ensemble, pas séparément. Le contenu final est une fonction des entrées : son SHA-256 est imprimé quand un fichier est écrit, et un test vérifie que deux runs partis du même fichier laissent le même fichier.
3. **Le fichier de flash est demandé, brut et exact.** `--flash-file <chemin>` le désigne ; sans cette option rien n'est conservé, et le résumé dit combien de secteurs de 4 Kio ont été modifiés et perdus. Le fichier est l'image de la puce et rien d'autre : pas d'en-tête, exactement la taille de la flash du profil, donc lisible par `esptool.py` et comparable par `sha256sum`. Un fichier d'une autre taille est refusé avec les deux tailles ; il n'est jamais complété ni coupé, même sous `--reset-flash` : un fichier d'une autre taille n'est pas la flash de cet appareil, et une image de firmware donnée par erreur à la mauvaise option doit survivre. Un lien symbolique n'est pas suivi.
4. **L'image de `--flash` n'est jamais écrite.** Elle est la graine : ce dont part un run quand aucun fichier de flash n'existe encore. Donnée avec un fichier de flash qui existe, elle est refusée avant le démarrage, car le run ne peut pas partir de deux flashs ; `--reset-flash` lève le refus : le run repart de la graine (ou d'une flash effacée) et le fichier est remplacé à la fin.
5. **Le fichier est lu au départ, écrit à la fin, et verrouillé entre les deux.**
   - Lu en entier au départ s'il existe, sa taille jugée avant toute lecture, sans jamais lire plus d'un octet au-delà.
   - Verrouillé pour tout le run par un fichier voisin, `<chemin>.lock`, sur lequel le processus tient un verrou exclusif (`File::try_lock`). Un second run sur le même fichier est refusé avant le démarrage. Le verrou est sur un voisin et non sur le fichier lui-même parce que le fichier peut ne pas exister au premier run et qu'il est remplacé à la fin : un verrou sur l'ancien fichier ne protégerait pas le nouveau. Le fichier de verrou n'est jamais supprimé (le supprimer permettrait à deux runs de tenir chacun « le » verrou) ; le système libère le verrou quand le processus meurt, quelle qu'en soit la cause.
   - Écrit quand le run s'achève, quelle que soit sa fin (limite de cycles ou de temps, premier Ctrl-C, arrêt de `--strict-mmio`, CPU en attente sans réveil possible, lecteur parti), si le fichier n'existait pas, s'il est remplacé, ou si le run a modifié la flash. Un run parti du fichier et qui n'a rien modifié le laisse tel quel.
   - Écrit par un fichier temporaire du même répertoire, vidé sur le disque puis renommé : le chemin désigne l'ancien contenu ou le nouveau, jamais un morceau.
   - Jamais écrit pendant le run. Un run tué, un second Ctrl-C ou une panne de l'hôte perdent donc ce que le run a écrit en flash : le fichier d'avant reste.
6. **Un fichier qui ne peut pas être écrit à la fin est une erreur du run.** Le message dit quoi, sur quel fichier, et que les écritures du run sont perdues ; le code de sortie est 2, quelle que soit la fin du run, sauf après un arrêt de `--strict-mmio`, qui garde 3 : ce code existe pour la CI et ne doit pas dépendre de l'état d'un répertoire (même règle que pour la console). Aucun nouveau code de sortie n'est créé.
7. **Le contenu sort de la machine en octets.** `Esp32C3::flash_contents` et `Esp32C3::flash_modified_sectors` suffisent à une interface pour conserver la flash ; le modèle ne connaît ni fichier ni chemin. La puce n'est construite que depuis `SocConfig::flash` : sa taille ne peut pas contredire la configuration.

## Conséquences

- **Enregistrement et rejeu (SP1b-4)** : un enregistrement doit porter, ou hacher, les quatre entrées du point 1 — et pour la flash le contenu de départ lui-même ou son SHA-256 avec le moyen de le retrouver —, puis les entrées datées quand elles existeront. Il ne porte ni chemin, ni heure de l'hôte. Un rejeu part d'un contenu, pas d'un fichier, et n'écrit rien. Le lockstep compare deux machines parties du même contenu.
- **Un crash perd les écritures du run.** C'est le prix d'un fichier toujours entier et d'un run sans entrée-sortie. Une sauvegarde en cours de run (à la demande, ou périodique) reste possible sans toucher à cette décision : elle écrirait le même contenu par le même renommage. Elle revient à l'interface (SP2), si le besoin se confirme.
- **Durées** : les opérations de flash s'achèvent aussitôt (ADR 0002, point 4). Le premier démarrage d'une disposition OTA, qui efface et programme `otadata`, est donc plus court ici que sur l'appareil d'une centaine de millisecondes ; les durées réelles reviennent au sous-projet 8.
- **Limites connues du fichier** : il porte le contenu de la puce, pas son registre d'état : les bits non volatils d'une pièce réelle (protection par blocs, QE) reviennent ici à leur valeur d'usine à chaque run. Le verrou est coopératif, il ne protège que d'un autre inkemu ; sur un système de fichiers sans verrous, le run est refusé. Un processus tué pendant l'écriture finale peut laisser un fichier temporaire (`.<nom>.<pid>.tmp`), qu'aucun run ne supprimera. Le fichier étant remplacé et non réécrit, ses permissions redeviennent celles par défaut. Le répertoire n'est pas vidé sur le disque après le renommage : après une panne de courant de l'hôte, le chemin peut encore désigner l'ancien contenu. Les tests du verrou et du répertoire disparu supposent un système de fichiers POSIX ; Windows revient au sous-projet 9.
- **Écarts avec la spec, à y reporter** : la section 5.1 dit que chaque appareil virtuel possède son fichier de flash. Dans la CLI, ce fichier est demandé par `--flash-file` et n'existe pas sans elle : la notion d'appareil virtuel doté d'une identité arrive avec l'interface (SP2), qui choisira où ranger ces fichiers. La « commande de remise à zéro explicite » est l'option `--reset-flash` de `inkemu run`, pas une commande à part. La section ne dit pas quand le fichier est écrit : à la fin du run.
- **Numérotation** : l'ADR du bootloader embarqué, attendu du plan SP1b-3c, prend le numéro suivant.
````

- [ ] **Step 6: Mutations**

| # | Mutation | Command | Test that fails |
|---|---|---|---|
| 1 | `main.rs`: `RunFlash::begin(flash.file, flash.reset, image, flash_size)` → `RunFlash::begin(None, …)` | A | the four `…_is_refused_before_boot…` tests of Step 2 |
| 2 | `main.rs`: remove `requires = "flash_file"` | A | `a_reset_of_the_flash_without_a_flash_file_is_a_usage_error` |
| 3 | `main.rs`: the answer of `run_flash.finish(&machine, &mut io::stderr().lock())` ignored (`let _ = …;`), and `let code = end.exit_code();` | B | `a_flash_file_whose_directory_vanished_during_the_run_exits_2_and_says_what_is_lost`, `a_flash_file_whose_directory_became_read_only_exits_2_and_leaves_no_file` |
| 4 | `main.rs`: `RunFlash::begin(flash.file, flash.reset, image, flash_size)` → `RunFlash::begin(flash.file, false, image, flash_size)` | B | `the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next` |
| 5 | `main.rs`: `Some(path) if run_flash.starts_from_the_file()` → `if !run_flash.starts_from_the_file()` | B | the same test |
| 6 | `main.rs`: `if run_flash.finish(&machine, …) {` → `if true {` | B | six tests, `a_run_without_a_flash_file_says_what_it_does_not_keep_and_leaves_the_image_as_it_is` among them |
| 7 | `crates/soc-esp32c3/src/flash.rs`, `erase`: remove `self.fill_erased(start, unit);` | B | `the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next`, `the_same_starting_flash_file_gives_the_same_trace_and_the_same_final_file` |
| 8 | `flash_state.rs`: `match lock.try_lock() {` → `match Ok::<(), TryLockError>(()) {` | B | `a_second_run_on_a_flash_file_is_refused_while_the_first_goes_on_and_ctrl_c_saves_it` |

Command A: `cargo test -p emu-cli --test run_cli > /tmp/m.log 2>&1; echo "exit=$?"`. Command B: `cargo test --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware > /tmp/m.log 2>&1; echo "exit=$?"`.

Row 7 is the only place where a test on the real ROM tells an erase that erases from one that does not: the second stage erases a sector that holds a byte.

Every test that starts `inkemu` gives it `--rom-dir` and a limit (`--max-cycles`, or a timeout of the test for the run that Ctrl-C ends): under mutations 2 and 8 a run that should have been refused starts, and without them it would download a ROM or never end.

- [ ] **Step 7: Format, lint, commit**

```bash
/usr/bin/git add docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md
/usr/bin/git commit -m "feat(cli): inkemu run --flash-file keeps the flash of a device between runs

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-cli docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md .github/workflows/ci.yml
```

---
## Task 11: The NVS restart counter example, as built and under the X4 table

The proof on bytes Espressif's code writes needs a firmware that stores something. ESP-IDF's `examples/storage/nvs/nvs_rw_value_cxx` opens NVS, reads `restart_counter`, increments it, commits, and calls `esp_restart()` after a countdown of ten seconds. It is built unmodified, next to `hello_world`, by the same container run, and merged twice: under its own single-application partition table, and under the stock table of the X4 (`tests/fixtures/partitions/x4-stock.bin`: `otadata` and two application slots), which makes the bootloader write `otadata` on the first boot. Neither the bootloader nor the application embeds the table, so the second image needs no second build.

This is the only task that needs Docker (`espressif/idf:v5.5.2`, pulled by the existing `tools/docker/esp-idf/Dockerfile`). Run the build **once**; never start two `docker run` at the same time.

**Files:**
- Modify: `tools/reference-firmware/in-container.sh` (rewritten around three functions)
- Modify: `tools/reference-firmware/build.sh` (mounts the partition fixtures, prints the three images)
- Create: `tools/reference-firmware/SHA256SUMS.inputs-nvs`
- Modify: `tools/reference-firmware/SHA256SUMS.output` (three lines)
- Modify: `tools/reference-firmware/README.md`
- Modify: `.github/workflows/ci.yml` (the cache key, the name of the verify step)

**Interfaces:**
- Consumes: `tools/reference-firmware/sdkconfig.defaults`, `SHA256SUMS.inputs` (unchanged), `tests/fixtures/partitions/x4-stock.bin`, `tools/conformance/versions.env`.
- Produces, in the output directory of `tools/reference-firmware/build.sh [output-dir]` (default `target/reference-firmware`):
  - `flash_hello.bin` (unchanged, `fc05ef5d…d8c39bf2`), `bootloader.nm`, `hello_world.nm`;
  - `flash_nvs.bin`, SHA-256 `c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f`;
  - `flash_nvs_ota.bin`, SHA-256 `87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec`;
  - `nvs_rw_value_cxx.nm`, the symbol table of the application.
  - Pinned on the way: `nvs_rw_value_cxx.bin` `44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2`, with the same bootloader and partition table as `hello_world` (SF §A, §G).
- Tasks 12 and 13 read the two images through `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH`.

The images are data: nothing in this task, and nothing in the repository, executes them on the host.

- [ ] **Step 1: The pinned hashes**

Create `tools/reference-firmware/SHA256SUMS.inputs-nvs`:

````text
601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3  bootloader/bootloader.bin
7f00b6c042a89b15b0cac534f82ed988caf29278ff5700b0c511eb1b5bb7c820  partition_table/partition-table.bin
44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2  nvs_rw_value_cxx.bin
````

````diff
--- a/tools/reference-firmware/SHA256SUMS.output
+++ b/tools/reference-firmware/SHA256SUMS.output
@@ -1 +1,3 @@
 fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2  flash_hello.bin
+c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f  flash_nvs.bin
+87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec  flash_nvs_ota.bin
````

- [ ] **Step 2: The script that runs in the container**

Replace `tools/reference-firmware/in-container.sh` by:

````bash
#!/usr/bin/env bash
# Runs inside the espressif/idf container (see build.sh). /ref is this directory and /partitions the
# partition-table fixtures of the repository, both read-only; /out receives the results. Nothing
# reaches /out until every merged image has passed its pinned hash: the images and the symbol
# tables are produced in a temporary directory, verified, and only then moved.
set -euo pipefail

# Runs one step with its output in a log file; on failure prints the tail of that log and stops.
step() {
  local name="$1" log="$2"
  shift 2
  if ! "$@" > "$log" 2>&1; then
    tail -n 60 "$log" >&2
    echo "error: $name failed; see the log above" >&2
    exit 1
  fi
}

# Builds the ESP-IDF example "$1" (a path under $IDF_PATH/examples), unmodified, with the reference
# configuration as its only one, checks its three binaries against the pinned hashes of "$2", and
# leaves the shell in its build directory. It says how long the build took: the cost of each
# example in a CI job is read in the job log.
build_example() {
  local example="$1" sums="$2"
  local name project started="$SECONDS"
  name="$(basename "$example")"
  project="/tmp/$name"
  rm -rf "$project"
  cp -r "$IDF_PATH/examples/$example" "$project"
  cp /ref/sdkconfig.defaults "$project/sdkconfig.defaults"
  cd "$project"
  step "idf.py set-target ($name)" "/tmp/set-target-$name.log" idf.py set-target esp32c3
  step "idf.py build ($name)" "/tmp/build-$name.log" idf.py build
  cd build
  if ! sha256sum -c "$sums"; then
    sha256sum bootloader/bootloader.bin partition_table/partition-table.bin "$name.bin" >&2
    echo "error: the build of $name is not the pinned one (hashes above); do not edit $(basename "$sums"): check the image tag, the ESP-IDF version and sdkconfig.defaults" >&2
    exit 1
  fi
  echo "$name: built and checked in $((SECONDS - started)) s"
}

# Merges the bootloader of the current build directory, the partition table "$2" and the
# application "$3" into the 16 MB flash image "$stage/$1".
merge() {
  local image="$1" table="$2" application="$3"
  step "esptool merge_bin ($image)" "/tmp/merge-$image.log" \
    esptool.py --chip esp32c3 merge_bin --fill-flash-size 16MB -o "$stage/$image" \
    --flash_mode dio --flash_freq 80m --flash_size 16MB \
    0x0 bootloader/bootloader.bin 0x8000 "$table" 0x10000 "$application"
}

# Writes the symbol table of each ELF file given (without its extension) to "$stage".
symbols() {
  local elf
  for elf in "$@"; do
    if ! riscv32-esp-elf-nm -n "$elf.elf" > "$stage/$(basename "$elf").nm" 2> /tmp/nm.log; then
      tail -n 20 /tmp/nm.log >&2
      echo "error: riscv32-esp-elf-nm failed on $elf.elf; see the log above" >&2
      exit 1
    fi
  done
}

stage="$(mktemp -d)"

build_example get-started/hello_world /ref/SHA256SUMS.inputs
merge flash_hello.bin partition_table/partition-table.bin hello_world.bin
symbols bootloader/bootloader hello_world

# The example that keeps a restart counter in NVS: the same bootloader and the same single-app
# table (its sums pin them again), once as built and once under the stock partition table of the
# X4, which has `otadata` and two application slots. Neither the bootloader nor the application
# embeds the table, so the second image needs no second build.
build_example storage/nvs/nvs_rw_value_cxx /ref/SHA256SUMS.inputs-nvs
merge flash_nvs.bin partition_table/partition-table.bin nvs_rw_value_cxx.bin
merge flash_nvs_ota.bin /partitions/x4-stock.bin nvs_rw_value_cxx.bin
symbols nvs_rw_value_cxx

if ! (cd "$stage" && sha256sum -c /ref/SHA256SUMS.output); then
  (cd "$stage" && sha256sum flash_hello.bin flash_nvs.bin flash_nvs_ota.bin) >&2
  tail -n 20 /tmp/merge-*.log >&2
  echo "error: a merged image is not the pinned one (hashes above); nothing was written to the output directory" >&2
  exit 1
fi
results=(flash_hello.bin flash_nvs.bin flash_nvs_ota.bin bootloader.nm hello_world.nm nvs_rw_value_cxx.nm)
(cd "$stage" && chown "$HOST_UID:$HOST_GID" "${results[@]}" && mv "${results[@]}" /out/)
````

- [ ] **Step 3: The script that starts it, and its README**

````diff
--- a/tools/reference-firmware/build.sh
+++ b/tools/reference-firmware/build.sh
@@ -1,8 +1,14 @@
 #!/usr/bin/env bash
-# Builds the reference flash image of the SP1b tests: the stock ESP-IDF 2nd-stage bootloader and the
-# hello_world example, merged into a 16 MB image. Everything third-party runs inside Docker; nothing
-# built here is committed. The three binaries and the merged image are checked against pinned
-# SHA-256 values (docs/research/sp1b-test-oracle.md, section 2): a mismatch stops the script.
+# Builds the reference flash images of the SP1b tests, each a 16 MB image whose bootloader is the
+# stock ESP-IDF 2nd-stage bootloader:
+#   flash_hello.bin    the hello_world example;
+#   flash_nvs.bin      the nvs_rw_value_cxx example, which keeps a restart counter in NVS;
+#   flash_nvs_ota.bin  the same application under the stock partition table of the X4
+#                      (tests/fixtures/partitions/x4-stock.bin), whose bootloader writes otadata.
+# Everything third-party runs inside Docker, in one container run; nothing built here is committed.
+# The binaries and the merged images are checked against pinned SHA-256 values
+# (docs/research/sp1b-test-oracle.md, section 2; docs/research/esp-idf-storage-flash-traffic.md,
+# sections A and G): a mismatch stops the script.
 #
 # Usage: tools/reference-firmware/build.sh [output-dir]      (default: target/reference-firmware)
 set -euo pipefail
@@ -12,11 +18,15 @@ out="${1:-target/reference-firmware}"
 mkdir -p "$out"
 out="$(cd "$out" && pwd)"
 docker build -q -t inkemu/esp-idf --build-arg ESP_IDF_VERSION="$ESP_IDF_VERSION" tools/docker/esp-idf > /dev/null
-# No network: the example has no managed component, and the build must not depend on anything that
-# is not in the pinned image. The container builds as its own user in its own file system (the
+# No network: the examples have no managed component, and the build must not depend on anything
+# that is not in the pinned image. The container builds as its own user in its own file system (the
 # conditions the pinned hashes were measured in) and hands over the results at the end.
 docker run --rm --network none \
   -e IDF_COMPONENT_MANAGER=0 -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
-  -v "$PWD/tools/reference-firmware:/ref:ro" -v "$out:/out" \
+  -v "$PWD/tools/reference-firmware:/ref:ro" \
+  -v "$PWD/tests/fixtures/partitions:/partitions:ro" \
+  -v "$out:/out" \
   inkemu/esp-idf bash /ref/in-container.sh
 echo "$out/flash_hello.bin"
+echo "$out/flash_nvs.bin"
+echo "$out/flash_nvs_ota.bin"
````

````diff
--- a/tools/reference-firmware/README.md
+++ b/tools/reference-firmware/README.md
@@ -1,21 +1,33 @@
 # Reference firmware of the SP1b tests
 
-`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image, the stock ESP-IDF
-2nd-stage bootloader and the `hello_world` example for the ESP32-C3, checks the three binaries
-against `SHA256SUMS.inputs`, merges them into a 16 MB flash image and checks it against
-`SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in `tools/conformance/versions.env`.
+`build.sh [output-dir]` builds, inside the pinned `espressif/idf` Docker image and in one container
+run, the stock ESP-IDF 2nd-stage bootloader and two unmodified ESP-IDF examples for the ESP32-C3,
+checks the binaries of each build against its pinned hashes, merges them into 16 MB flash images
+and checks the images against `SHA256SUMS.output`. The ESP-IDF version is `ESP_IDF_VERSION` in
+`tools/conformance/versions.env`.
 
-Nothing built here is committed and nothing is executed on the host: the image is data for inkemu.
-`bootloader.nm` and `hello_world.nm` are symbol tables (text) that turn a program counter printed by
-inkemu into a function name: the last symbol whose address is not above the PC.
+| Image | Application | Partition table | Pins of its build |
+|---|---|---|---|
+| `flash_hello.bin` | `examples/get-started/hello_world` | the single-app table of the build | `SHA256SUMS.inputs` |
+| `flash_nvs.bin` | `examples/storage/nvs/nvs_rw_value_cxx`: it keeps a restart counter in NVS, prints it, and restarts every eleven seconds | the single-app table of the build | `SHA256SUMS.inputs-nvs` |
+| `flash_nvs_ota.bin` | the same application | `tests/fixtures/partitions/x4-stock.bin`, the stock table of the X4: `otadata` and two application slots, so the bootloader writes `otadata` on the first boot | `SHA256SUMS.inputs-nvs` |
 
-Run the tests that need the image:
+Nothing built here is committed and nothing is executed on the host: the images are data for inkemu.
+`bootloader.nm`, `hello_world.nm` and `nvs_rw_value_cxx.nm` are symbol tables (text) that turn a
+program counter printed by inkemu into a function name: the last symbol whose address is not above
+the PC.
+
+Run the tests that need the images:
 
     tools/reference-firmware/build.sh
     cargo run --release -p emu-cli -- rom fetch --all --rom-dir /tmp/inkemu-rom
-    INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin \
+    INKEMU_ROM_ELF_DIR=/tmp/inkemu-rom \
+      INKEMU_REF_FLASH=target/reference-firmware/flash_hello.bin \
+      INKEMU_NVS_FLASH=target/reference-firmware/flash_nvs.bin \
+      INKEMU_NVS_OTA_FLASH=target/reference-firmware/flash_nvs_ota.bin \
       cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
 
-The build is reproducible (same hashes from two clean builds and from another path): see
-`docs/research/sp1b-test-oracle.md`, section 2. If the hashes differ, the image, the ESP-IDF version
-or `sdkconfig.defaults` changed: fix that, never the pinned values.
+The builds are reproducible (same hashes from clean builds, from another path and with the compiler
+cache off): see `docs/research/sp1b-test-oracle.md`, section 2, and
+`docs/research/esp-idf-storage-flash-traffic.md`, section A.2. If the hashes differ, the image, the
+ESP-IDF version or `sdkconfig.defaults` changed: fix that, never the pinned values.
````

- [ ] **Step 4: Build, once**

```bash
tools/reference-firmware/build.sh /tmp/reference-firmware > /tmp/t11-build.log 2> /tmp/t11-build.err; echo "exit=$?"; grep -E "OK$|built and checked" /tmp/t11-build.log
```

Expected: `exit=0` and, in this order: three `: OK` lines and `hello_world: built and checked in … s`, three `: OK` lines and `nvs_rw_value_cxx: built and checked in … s`, then `flash_hello.bin: OK`, `flash_nvs.bin: OK`, `flash_nvs_ota.bin: OK`. On the host this plan was written on the run took 74 s once the image was pulled (34 s and 32 s for the two examples).

A hash that does not match stops the script before anything reaches the output directory. Do not edit a pinned hash to make the build pass: the cause is the image tag, the ESP-IDF version or `sdkconfig.defaults`.

- [ ] **Step 5: The check the CI runs on every job, cache hit or not**

```bash
(cd /tmp/reference-firmware && sha256sum -c "$OLDPWD/tools/reference-firmware/SHA256SUMS.output") > /tmp/t11-sums.log 2>&1; echo "exit=$?"
```

Expected: `exit=0`. Then see it refuse: copy the three images to a scratch directory, change one byte of one copy, and run the same `sha256sum -c` there:

```bash
mkdir /tmp/t11-tamper && cp /tmp/reference-firmware/flash_*.bin /tmp/t11-tamper/
printf '\x00' | dd of=/tmp/t11-tamper/flash_nvs_ota.bin bs=1 seek=57344 conv=notrunc status=none
(cd /tmp/t11-tamper && sha256sum -c "$OLDPWD/tools/reference-firmware/SHA256SUMS.output") > /tmp/t11-tamper.log 2>&1; echo "exit=$?"; rm -r /tmp/t11-tamper
```

Expected: `exit=1`, and the log names `flash_nvs_ota.bin` as the one that failed.

- [ ] **Step 6: The CI job**

The cache of the firmware must be rebuilt when the partition fixture changes, and the verify step now checks three images. (The tests that use the two new images, their environment variables and their counts come with Tasks 12 and 13.)

````diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -95,9 +95,11 @@ jobs:
   reference-boot:
     # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM
     # and runs it through a whole life: the application, its countdown, the restart it asks for
-    # and the second boot. The firmware is built here, inside the pinned ESP-IDF image, and
-    # checked against pinned SHA-256 values; it is never stored in this repository. The cache
-    # saves the build and replaces no check: the image is re-hashed on every run.
+    # and the second boot. A second example, nvs_rw_value_cxx, keeps a restart counter in NVS:
+    # it is built next to the first, once under its own partition table and once under the stock
+    # table of the X4. The firmware is built here, inside the pinned ESP-IDF image, and checked
+    # against pinned SHA-256 values; it is never stored in this repository. The cache saves the
+    # build and replaces no check: the images are re-hashed on every run.
     runs-on: ubuntu-24.04
     timeout-minutes: 45
     steps:
@@ -113,7 +115,7 @@ jobs:
         uses: actions/cache@v6
         with:
           path: ${{ runner.temp }}/reference-firmware
-          key: reference-firmware-${{ hashFiles('tools/reference-firmware/**', 'tools/docker/esp-idf/Dockerfile', 'tools/conformance/versions.env') }}
+          key: reference-firmware-${{ hashFiles('tools/reference-firmware/**', 'tools/docker/esp-idf/Dockerfile', 'tools/conformance/versions.env', 'tests/fixtures/partitions/x4-stock.bin') }}
       # The build pulls the pinned ESP-IDF image, which takes 12.5 GB of disk once extracted
       # (measured on a development host), and GitHub only guarantees 14 GB to a hosted Linux runner:
       # without room made first, the pull or the Rust build after it can run out of disk. The
@@ -129,7 +131,7 @@ jobs:
       - name: Build the reference firmware
         if: steps.firmware.outputs.cache-hit != 'true'
         run: tools/reference-firmware/build.sh "$RUNNER_TEMP/reference-firmware"
-      - name: Verify the reference image
+      - name: Verify the reference images
         working-directory: ${{ runner.temp }}/reference-firmware
         run: sha256sum -c "$GITHUB_WORKSPACE/tools/reference-firmware/SHA256SUMS.output"
       - name: Fetch and verify the mask ROMs
````

- [ ] **Step 7: Run the suite, commit**

```bash
cargo test --workspace > /tmp/t11.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t11.log
/usr/bin/git add tools/reference-firmware/SHA256SUMS.inputs-nvs
/usr/bin/git commit -m "build(reference-firmware): the NVS restart counter example, as built and under the X4 table

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- tools/reference-firmware .github/workflows/ci.yml
```

Expected: `exit=0` and `927` (no test is added: the proof of this task is Steps 4 and 5).

---
## Task 12: Three lives of the NVS example: the counter survives, the bytes are the measured ones

The milestone, on Espressif's code: the example of Task 11 runs three lives in one strict run. The second and the third life print the counter the life before wrote; the programs are the eleven measured ones, byte for byte and in order (SF §B.2); the flash ends with the measured SHA-256. The same under the X4 partition table, where the bootloader writes `otadata` once. One more thing is pinned here, per phase of a first boot: the commands the controller sends, so that the artefact of QEMU's chip (a write disable after every write) cannot come back unseen.

**Files:**
- Modify: `crates/soc-esp32c3/src/periph/spi_mem.rs` (a counter per opcode)
- Modify: `crates/soc-esp32c3/src/machine.rs` (`flash_commands_sent`)
- Modify: `crates/soc-esp32c3/tests/reference_boot.rs` (5 ignored tests)
- Modify: `.github/workflows/ci.yml` (the two images, the count 7 → 12)

**Interfaces:**
- Consumes: `Esp32C3::{take_flash_writes, flash_writes_dropped}` (Task 3), `flash_contents`, `flash_modified_sectors` (Task 7), `Diagnostic::StaleFlashWindow` (Task 6); the images of Task 11 through `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH`.
- Produces:
  - `pub fn Spi1::commands_sent(&self, opcode: u8) -> u64` and `pub fn Esp32C3::flash_commands_sent(&self, opcode: u8) -> u64`: the commands the controller sent to the chip since power-on, those the chip refused included. They belong to the run: no reset clears them.
  - In `tests/reference_boot.rs`: `pinned_image(variable, file, sha256) -> Vec<u8>` (an image is accepted only with its pinned SHA-256), and `Run` gains `flash`, `flash_modified_sectors` and `flash_commands`.

**Bound.** The counters are `[u64; 256]`, one per value a command byte can hold: their size does not depend on what the firmware sends.

**What is asserted, and what is not.** The content hashes after each life (`cfab4ffe…`, `7ad0acf6…`, `fc23c7ec…` for the single-app image; `6c668205…` after three lives of the OTA image) are measured under QEMU and replayed (SF §B.2, §G). No test asserts a count of read-status commands (`05`): how many there are depends on how fast the chip answers. Write disables (`04`) are asserted to be **zero in the application phase** of a first boot, by difference between two points of one run; no absolute count of them is asserted. The number of resets after the last life is not asserted either: a run ends on the last line of a life, a few milliseconds before or after the restart that follows it.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/soc-esp32c3/tests/reference_boot.rs
+++ b/crates/soc-esp32c3/tests/reference_boot.rs
@@ -158,9 +158,15 @@ fn rom() -> RomImage {
 /// every test boots it, so a wrong image fails here with its real cause instead of as a
 /// golden-log difference.
 fn reference_flash() -> Vec<u8> {
-    let path = std::env::var("INKEMU_REF_FLASH").unwrap_or_else(|e| {
+    pinned_image("INKEMU_REF_FLASH", "flash_hello.bin", REFERENCE_SHA256)
+}
+
+/// The image `file` of `tools/reference-firmware/build.sh`, named by the environment variable
+/// `variable` and accepted only if its SHA-256 is `sha256`.
+fn pinned_image(variable: &str, file: &str, sha256: &str) -> Vec<u8> {
+    let path = std::env::var(variable).unwrap_or_else(|e| {
         panic!(
-            "INKEMU_REF_FLASH is not usable ({e}); build the image with `tools/reference-firmware/build.sh` and set the variable to the flash_hello.bin it prints"
+            "{variable} is not usable ({e}); build the image with `tools/reference-firmware/build.sh` and set the variable to the {file} it prints"
         )
     });
     let image = std::fs::read(&path).unwrap_or_else(|e| {
@@ -170,8 +176,8 @@ fn reference_flash() -> Vec<u8> {
     });
     let digest = common::hex(&Sha256::digest(&image));
     assert_eq!(
-        digest, REFERENCE_SHA256,
-        "{path} is not the pinned reference image; rebuild it with `tools/reference-firmware/build.sh`, which verifies the pins"
+        digest, sha256,
+        "{path} is not the pinned {file}; rebuild it with `tools/reference-firmware/build.sh`, which verifies the pins"
     );
     image
 }
@@ -191,6 +197,11 @@ struct Run {
     diagnostics: Vec<Diagnostic>,
     /// Every program, erase and status write the flash chip performed, in order.
     flash_writes: Vec<FlashWrite>,
+    /// The whole content of the flash when the run ended.
+    flash: Vec<u8>,
+    flash_modified_sectors: u32,
+    /// How many flash commands of each opcode were sent to the chip, indexed by opcode.
+    flash_commands: Vec<u64>,
     stop: StopReason,
     trace_hash: Option<u64>,
     resets: u64,
@@ -270,6 +281,11 @@ fn run_image(image: Vec<u8>, config: SocConfig, done: impl Fn(&str, &[Diagnostic
         console: text(&uart0),
         diagnostics,
         flash_writes,
+        flash: machine.flash_contents(),
+        flash_modified_sectors: machine.flash_modified_sectors(),
+        flash_commands: (0..=u8::MAX)
+            .map(|opcode| machine.flash_commands_sent(opcode))
+            .collect(),
         stop,
         trace_hash: machine.trace_hash(),
         resets: machine.resets(),
@@ -626,3 +642,331 @@ fn two_life_cycles_are_identical_down_to_the_trace_hash_across_the_reset() {
     );
     assert_eq!(first.console, second.console);
 }
+
+// ---------------------------------------------------------------------------------------------
+// The second reference firmware: ESP-IDF's `examples/storage/nvs/nvs_rw_value_cxx`, unmodified.
+// Each of its lives reads a restart counter from NVS, writes it back incremented, counts down
+// for eleven seconds and restarts (report SF §A.1). Only the second life proves that a write was
+// kept: the first prints the same lines whether the flash took the bytes or not.
+// ---------------------------------------------------------------------------------------------
+
+/// `flash_nvs.bin`: the example behind its own single-app partition table (SF §A.3).
+const NVS_SHA256: &str = "c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f";
+/// `flash_nvs_ota.bin`: the same application behind the stock partition table of the X4.
+const NVS_OTA_SHA256: &str = "87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec";
+/// The whole flash after each life of `flash_nvs.bin`, measured under QEMU and replayed (SF §B.2).
+const NVS_AFTER_LIFE_1: &str = "cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b";
+const NVS_AFTER_LIFE_2: &str = "7ad0acf62e5a4e20aa6d47be0f1d922a7b89cb204941feac1ab2fea71363b709";
+const NVS_AFTER_LIFE_3: &str = "fc23c7ecb700e966c0ef9db3becc6b73e37324b5a21e5380519fe597542143f5";
+/// The whole flash after three lives of `flash_nvs_ota.bin` (SF §B.2).
+const NVS_OTA_AFTER_LIFE_3: &str =
+    "6c668205970fc21e7330b840b44e47ca1a2a245f22d93ef1cb0f239a3b89de5b";
+
+fn nvs_flash() -> Vec<u8> {
+    pinned_image("INKEMU_NVS_FLASH", "flash_nvs.bin", NVS_SHA256)
+}
+
+fn nvs_ota_flash() -> Vec<u8> {
+    pinned_image("INKEMU_NVS_OTA_FLASH", "flash_nvs_ota.bin", NVS_OTA_SHA256)
+}
+
+/// The configuration of every run of the example: nothing unmodelled is let through.
+fn strict() -> SocConfig {
+    SocConfig {
+        strict_mmio: true,
+        ..SocConfig::default()
+    }
+}
+
+/// The last line of a life of the example: the restart follows.
+const RESTARTING_NOW: &str = "Restarting now.\n";
+
+/// Ends a run when the example has printed the last line of its `lives`-th life. Its writes are
+/// done by then: the last one is eleven seconds before the restart (SF §B.2).
+fn lives(lives: usize) -> impl Fn(&str, &[Diagnostic]) -> bool {
+    move |console, _| console.matches(RESTARTING_NOW).count() >= lives
+}
+
+fn sha256(bytes: &[u8]) -> String {
+    common::hex(&Sha256::digest(bytes))
+}
+
+fn program(address: u32, data: &[u8]) -> FlashWrite {
+    FlashWrite::Program {
+        address,
+        data: data.to_vec(),
+    }
+}
+
+/// The NVS entry that holds `restart_counter = value` (SF §B.2; `IDF:nvs_flash/src/nvs_page.cpp`:
+/// namespace 1, type i32, span 1, chunk index `FF`, CRC, the key in 16 bytes, the value in 8).
+fn counter_entry(crc: [u8; 4], value: u8) -> Vec<u8> {
+    let mut entry = vec![0x01, 0x14, 0x01, 0xFF];
+    entry.extend(crc);
+    entry.extend(b"restart_counter\0");
+    entry.extend([value, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
+    entry
+}
+
+/// Where NVS keeps the state of the 126 entries of its first page, two bits each: the word the
+/// example programs again and again, clearing bits, with no erase (SF §B.2, §G assertion 3).
+const ENTRY_STATES: u32 = 0x9020;
+
+/// Every program of the three lives of the example on an erased NVS partition, in order: the
+/// eleven rows of SF §B.2, measured under QEMU. Five on the first life (the page header, the
+/// namespace entry, the counter), three on each life after (the new counter, then the old one
+/// marked erased).
+fn the_programs_of_the_three_lives() -> [Vec<FlashWrite>; 3] {
+    let mut page_header = vec![0xFE, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0, 0xFE];
+    page_header.extend([0xFF; 19]);
+    page_header.extend([0x84, 0x2D, 0xBA, 0xB9]);
+    let mut namespace = vec![0x00, 0x01, 0x01, 0xFF, 0x09, 0xA9, 0x50, 0x07];
+    namespace.extend(b"storage");
+    namespace.extend([0; 9]);
+    namespace.push(0x01);
+    namespace.extend([0xFF; 7]);
+    let states = |low: u8| program(ENTRY_STATES, &[low, 0xFF, 0xFF, 0xFF]);
+    [
+        vec![
+            program(0x9000, &page_header),
+            program(0x9040, &namespace),
+            states(0xFE),
+            program(0x9060, &counter_entry([0x81, 0xA8, 0xFF, 0x42], 1)),
+            states(0xFA),
+        ],
+        vec![
+            program(0x9080, &counter_entry([0x62, 0xAF, 0x70, 0xCC], 2)),
+            states(0xEA),
+            states(0xE2),
+        ],
+        vec![
+            program(0x90A0, &counter_entry([0xFC, 0xAF, 0xDA, 0x00], 3)),
+            states(0xA2),
+            states(0x82),
+        ],
+    ]
+}
+
+/// What the journal of a strict run of the example may hold: the restart it asks for, and
+/// nothing else. No unmodelled register, no refused flash command, no window read that may
+/// differ from the device, and no CPU exception.
+///
+/// The restart is one entry however many lives the run crossed: the journal lists a reset once
+/// per cause and PC. A run that ends on the last line of a life may or may not have reached the
+/// restart that follows it, a few milliseconds later: no entry at all is accepted too.
+fn assert_nothing_but_the_restart_is_reported(diagnostics: &[Diagnostic]) {
+    assert!(diagnostics.len() <= 1, "{diagnostics:#?}");
+    for diagnostic in diagnostics {
+        assert!(
+            matches!(
+                diagnostic,
+                Diagnostic::Reset {
+                    cause: ResetCause::SoftwareCpu,
+                    pc: RESTART_PC,
+                    ..
+                }
+            ),
+            "{diagnostic}"
+        );
+    }
+}
+
+/// The milestone of the flash writes: three lives of the example in strict mode. The counter the
+/// second and the third life print is the one the life before wrote; the programs are the
+/// measured ones, byte for byte and in order; the flash ends as the measured image.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
+fn the_nvs_example_counts_its_restarts_over_three_lives_and_programs_the_measured_bytes() {
+    let run = run_image(nvs_flash(), strict(), lives(3));
+    assert!(
+        matches!(run.stop, StopReason::CycleBudget | StopReason::Asleep),
+        "a strict run of the example does not stop: {:?}\nconsole:\n{}",
+        run.stop,
+        run.console
+    );
+    assert_lines_in_order(
+        &run.console,
+        &[
+            "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "Opening Non-Volatile Storage (NVS) handle... Done",
+            "Reading restart counter from NVS ... The value is not initialized yet!",
+            "Updating restart counter in NVS ... Done",
+            "Committing updates in NVS ... Done",
+            "Restarting in 10 seconds...",
+            "Restarting in 0 seconds...",
+            "Restarting now.",
+            "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "Reading restart counter from NVS ... Done",
+            "Restart counter = 1",
+            "Committing updates in NVS ... Done",
+            "Restarting now.",
+            "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
+            "Reading restart counter from NVS ... Done",
+            "Restart counter = 2",
+            "Committing updates in NVS ... Done",
+            "Restarting now.",
+        ],
+    );
+    assert_eq!(
+        count_lines(
+            &run.console,
+            "Reading restart counter from NVS ... The value is not initialized yet!"
+        ),
+        1,
+        "only the first life finds no counter"
+    );
+    assert_eq!(run.flash_writes, the_programs_of_the_three_lives().concat());
+    assert_eq!(sha256(&run.flash), NVS_AFTER_LIFE_3);
+    assert_eq!(run.flash_modified_sectors, 1, "the first page of NVS");
+    assert_eq!(
+        run.flash[ENTRY_STATES as usize], 0x82,
+        "FF to 82 with no erase"
+    );
+    assert!(run.resets >= 2, "two restarts were crossed: {}", run.resets);
+    assert_nothing_but_the_restart_is_reported(&run.diagnostics);
+}
+
+/// The same three lives behind the stock partition table of the X4: the bootloader writes
+/// `otadata` on the first boot, and on that boot only; the application then writes the same NVS
+/// bytes at the same addresses.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_OTA_FLASH"]
+fn the_nvs_example_behind_the_stock_x4_table_writes_otadata_once_and_counts_its_restarts() {
+    let run = run_image(nvs_ota_flash(), strict(), lives(3));
+    assert!(
+        matches!(run.stop, StopReason::CycleBudget | StopReason::Asleep),
+        "a strict run of the example does not stop: {:?}\nconsole:\n{}",
+        run.stop,
+        run.console
+    );
+    assert_lines_in_order(
+        &run.console,
+        &[
+            NO_FACTORY_IMAGE,
+            OTADATA_SET,
+            "Reading restart counter from NVS ... The value is not initialized yet!",
+            "Restarting now.",
+            "Restart counter = 1",
+            "Restarting now.",
+            "Restart counter = 2",
+            "Restarting now.",
+        ],
+    );
+    assert_eq!(count_lines(&run.console, NO_FACTORY_IMAGE), 1);
+    assert_eq!(count_lines(&run.console, OTADATA_SET), 1);
+    let mut expected = the_otadata_update().to_vec();
+    expected.extend(the_programs_of_the_three_lives().concat());
+    assert_eq!(run.flash_writes, expected);
+    assert_eq!(sha256(&run.flash), NVS_OTA_AFTER_LIFE_3);
+    assert_eq!(run.flash_modified_sectors, 2, "NVS and otadata");
+    assert_nothing_but_the_restart_is_reported(&run.diagnostics);
+}
+
+/// The flash a first life leaves is the measured one, and a machine built from it goes on
+/// counting: a power-on, not a restart, and the counter of the life before. This is what a
+/// flash file kept between two runs relies on.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
+fn a_cold_start_from_the_flash_the_first_life_left_goes_on_counting() {
+    let [first_life, second_life, _] = the_programs_of_the_three_lives();
+    let first = run_image(nvs_flash(), strict(), lives(1));
+    assert_eq!(first.flash_writes, first_life);
+    assert_eq!(sha256(&first.flash), NVS_AFTER_LIFE_1);
+    assert_nothing_but_the_restart_is_reported(&first.diagnostics);
+
+    let second = run_image(first.flash, strict(), lives(1));
+    // The ROM lines of the reference image: the bootloader is the same binary.
+    assert!(
+        second.console.starts_with(ROM_LINES),
+        "a power-on:\n{}",
+        second.console
+    );
+    assert_lines_in_order(
+        &second.console,
+        &[
+            "Reading restart counter from NVS ... Done",
+            "Restart counter = 1",
+            "Restarting now.",
+        ],
+    );
+    assert_eq!(
+        count_lines(
+            &second.console,
+            "Reading restart counter from NVS ... The value is not initialized yet!"
+        ),
+        0
+    );
+    assert_eq!(second.flash_writes, second_life);
+    assert_eq!(sha256(&second.flash), NVS_AFTER_LIFE_2);
+    assert_nothing_but_the_restart_is_reported(&second.diagnostics);
+}
+
+/// Determinism with a flash that is written: two runs from the same content are the same run,
+/// down to the trace hash, and leave the same flash.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_OTA_FLASH"]
+fn three_lives_of_the_nvs_example_are_the_same_run_twice_and_leave_the_same_flash() {
+    let config = SocConfig {
+        trace_hash: true,
+        ..strict()
+    };
+    let run = || run_image(nvs_ota_flash(), config, lives(3));
+    let (first, second) = (run(), run());
+    assert_eq!(first.console.matches(RESTARTING_NOW).count(), 3);
+    assert!(first.trace_hash.is_some());
+    assert_eq!(
+        (first.trace_hash, first.cycles, first.device_time_nanos),
+        (second.trace_hash, second.cycles, second.device_time_nanos)
+    );
+    assert_eq!(first.console, second.console);
+    assert_eq!(first.flash_writes, second.flash_writes);
+    assert_eq!(sha256(&first.flash), sha256(&second.flash));
+    assert_eq!(sha256(&first.flash), NVS_OTA_AFTER_LIFE_3);
+}
+
+/// The flash commands of a first boot, by phase, against the list measured under QEMU (report FW
+/// §A.4, SF §B.1): the bootloader's `otadata` update is two write enables, one sector erase and
+/// one program; the application's five programs are five write enables and five programs.
+///
+/// QEMU's chip keeps its write-enable latch set after a program, so ESP-IDF sends it a write
+/// disable (`04`) after each one: 142 of them in FW §A.4. A chip clears the latch when the
+/// operation completes, and so does the model: the application sends none here. Status reads
+/// (`05`) are not counted: how many there are depends on how fast the chip answers.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_OTA_FLASH"]
+fn a_first_boot_sends_one_write_enable_per_write_and_the_application_no_write_disable() {
+    const WRITE_ENABLE: usize = 0x06;
+    const WRITE_DISABLE: usize = 0x04;
+    const PROGRAM: usize = 0x02;
+    const SECTOR_ERASE: usize = 0x20;
+    const DUAL_IO_READ: usize = 0xBB;
+    /// Erases of a block or of the chip, and the status write: none is sent by this firmware.
+    const NEVER_SENT: [usize; 5] = [0xD8, 0x52, 0xC7, 0x60, 0x01];
+    // The machine is deterministic: the first run is the beginning of the second.
+    let bootloader = run_image(nvs_ota_flash(), strict(), |console, _| {
+        console.contains(FIRST_APPLICATION_LINE.trim_start_matches("I (#) "))
+    });
+    let life = run_image(nvs_ota_flash(), strict(), lives(1));
+    let (before, after) = (&bootloader.flash_commands, &life.flash_commands);
+    assert_eq!(bootloader.flash_writes, the_otadata_update());
+    assert_eq!(
+        [before[WRITE_ENABLE], before[SECTOR_ERASE], before[PROGRAM]],
+        [2, 1, 1],
+        "ROM and bootloader: the otadata update"
+    );
+    let application = |opcode: usize| after[opcode] - before[opcode];
+    assert_eq!(
+        [
+            application(WRITE_ENABLE),
+            application(PROGRAM),
+            application(SECTOR_ERASE),
+            application(WRITE_DISABLE),
+        ],
+        [5, 5, 0, 0],
+        "application: five times a write enable and a program, and no write disable"
+    );
+    assert!(application(DUAL_IO_READ) > 0, "it reads NVS through SPI1");
+    for opcode in NEVER_SENT {
+        assert_eq!(after[opcode], 0, "{opcode:#04x}");
+    }
+}
````

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -937,6 +955,42 @@ mod tests {
         );
     }
 
+    #[test]
+    fn every_command_sent_to_the_chip_is_counted_under_its_opcode_for_the_whole_run() {
+        let ctx = &mut Ctx::default();
+        let mut spi = spi1();
+        let sent =
+            |spi: &Spi1| [0x05, 0x06, 0x02, 0x04, 0x9F, 0x35].map(|op| spi.commands_sent(op));
+        assert_eq!(sent(&spi), [0; 6]);
+        // One program as ESP-IDF's driver sends it to a chip whose latch clears at completion:
+        // status, write enable, status, program, status (report SF §B.1). No write disable.
+        spi.write(ctx, CMD, CMD_RDSR);
+        spi.write(ctx, CMD, CMD_WREN);
+        spi.write(ctx, CMD, CMD_RDSR);
+        spi.write(ctx, W0, 0xFFFF_FFFE);
+        spi.write(ctx, ADDR, 0x0400_9020);
+        spi.write(ctx, CMD, CMD_PP);
+        spi.write(ctx, CMD, CMD_RDSR);
+        assert_eq!(sent(&spi), [3, 1, 1, 0, 0, 0]);
+        // A user transfer counts under the opcode of `USER2`; one the chip refuses was sent too.
+        user_read(ctx, &mut spi, 0x9F, 24);
+        user_read(ctx, &mut spi, 0x35, 8);
+        assert_eq!(
+            ctx.take_unsupported(),
+            Some(Unsupported::FlashCommand { opcode: 0x35 })
+        );
+        assert_eq!(sent(&spi), [3, 1, 1, 0, 1, 1]);
+        // A program of no byte is refused by the controller: nothing was sent to the chip.
+        spi.write(ctx, ADDR, 0x9020);
+        spi.write(ctx, CMD, CMD_PP);
+        assert!(ctx.take_unsupported().is_some());
+        assert_eq!(sent(&spi), [3, 1, 1, 0, 1, 1]);
+        // The counts are those of the run: a reset of the controller keeps them.
+        spi.reset();
+        spi.write(ctx, CMD, 1 << 29); // FLASH_WRDI
+        assert_eq!(sent(&spi), [3, 1, 1, 1, 1, 1]);
+    }
+
     #[test]
     fn unknown_user_opcode_is_reported() {
         let ctx = &mut Ctx::default();
````

- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p soc-esp32c3 > /tmp/t12-red.log 2>&1; echo "exit=$?"; grep -m2 "^error\[" /tmp/t12-red.log
```

Expected: `exit=101`; ``error[E0599]: no method named `commands_sent` found for struct `Spi1` `` (the unit tests) and ``no method named `flash_commands_sent` found for struct `Esp32C3` `` (`tests/reference_boot.rs`).

- [ ] **Step 3: The counters**

````diff
--- a/crates/soc-esp32c3/src/periph/spi_mem.rs
+++ b/crates/soc-esp32c3/src/periph/spi_mem.rs
@@ -241,8 +241,15 @@ pub struct Spi1 {
     /// What the commands of the run wrote to the chip. It belongs to the run, not to the
     /// controller: a reset of the controller keeps it.
     journal: FlashJournal,
+    /// How many commands of each opcode the controller sent to the chip, refused ones included.
+    /// Like the journal it belongs to the run. One counter per opcode a byte can hold: its size
+    /// does not depend on what the firmware sends.
+    sent: [u64; OPCODES],
 }
 
+/// The opcodes a command byte can hold.
+const OPCODES: usize = 256;
+
 impl Spi1 {
     #[must_use]
     pub fn new(flash: Box<dyn FlashDevice>) -> Spi1 {
@@ -250,6 +257,7 @@ impl Spi1 {
             registers: Spi1::reset_registers(),
             flash,
             journal: FlashJournal::default(),
+            sent: [0; OPCODES],
         }
     }
 
@@ -284,6 +292,13 @@ impl Spi1 {
         self.journal.dropped()
     }
 
+    /// How many commands of `opcode` the controller sent to the chip since power-on, those the
+    /// chip model refused included.
+    #[must_use]
+    pub fn commands_sent(&self, opcode: u8) -> u64 {
+        self.sent.get(usize::from(opcode)).copied().unwrap_or(0)
+    }
+
     fn register(&self, offset: u32) -> u32 {
         self.registers.read(offset).unwrap_or(0)
     }
@@ -298,6 +313,9 @@ impl Spi1 {
         data_in_len: usize,
     ) -> Option<Vec<u8>> {
         let mut data_in = vec![0u8; data_in_len.min(DATA_BYTES)];
+        if let Some(count) = self.sent.get_mut(usize::from(command.opcode)) {
+            *count += 1;
+        }
         let before = self.flash.generation();
         match self.flash.execute(command, &mut data_in) {
             Ok(written) => {
````

````diff
--- a/crates/soc-esp32c3/src/machine.rs
+++ b/crates/soc-esp32c3/src/machine.rs
@@ -505,6 +505,15 @@ impl Esp32C3 {
         self.bus.periph.spi1.flash_writes_dropped()
     }
 
+    /// How many flash commands of `opcode` the SPI1 controller sent to the chip since power-on
+    /// (`0x02` page program, `0x06` write enable, `0x04` write disable, `0x05` read status, and
+    /// so on), those the chip model refused included: what a test compares with the commands a
+    /// firmware sends to a real chip.
+    #[must_use]
+    pub fn flash_commands_sent(&self, opcode: u8) -> u64 {
+        self.bus.periph.spi1.commands_sent(opcode)
+    }
+
     /// The whole content of the flash chip as it is now, erased bytes included: exactly
     /// [`FlashConfig::size`](crate::flash::FlashConfig::size) bytes, at most 16 MiB whatever the
     /// firmware did. It is what a caller keeps to let a later run go on from this one: given
````

- [ ] **Step 4: Run the suite and the ignored tests; the CI job**

```bash
cargo test --workspace > /tmp/t12.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t12.log
tools/ci/run-ignored-tests.sh 12 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t12-i3.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` and `928`; `exit=0`. On the host this plan was written on, the 12 tests take 8.4 s in a release build (2.7 s for the 7 of before).

````diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -140,7 +140,9 @@ jobs:
         env:
           INKEMU_ROM_ELF_DIR: ${{ runner.temp }}/rom
           INKEMU_REF_FLASH: ${{ runner.temp }}/reference-firmware/flash_hello.bin
+          INKEMU_NVS_FLASH: ${{ runner.temp }}/reference-firmware/flash_nvs.bin
+          INKEMU_NVS_OTA_FLASH: ${{ runner.temp }}/reference-firmware/flash_nvs_ota.bin
         # Exact test counts, as in rom-boot: see tools/ci/run-ignored-tests.sh.
         run: |
-          tools/ci/run-ignored-tests.sh 7 --release -p soc-esp32c3 --test reference_boot -- --ignored
+          tools/ci/run-ignored-tests.sh 12 --release -p soc-esp32c3 --test reference_boot -- --ignored
           tools/ci/run-ignored-tests.sh 2 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored
````

- [ ] **Step 5: Mutations on the real firmware**

Each row: apply the change, run `cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/m.log 2>&1; echo "exit=$?"`, see the tests fail, undo.

| # | Mutation | Tests that fail |
|---|---|---|
| 1 | `flash.rs`, `program`: remove `self.status &= !STATUS_WEL;` (the latch stays set, as on QEMU's chip) | `a_first_boot_sends_one_write_enable_per_write_and_the_application_no_write_disable`, and no other |
| 2 | `flash.rs`, `program`: `let programmed = *cell & byte;` → `let programmed = *cell; let _ = byte;` (a program stores nothing) | the four NVS tests that count, and `the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only` |
| 3 | `periph/spi_mem.rs`: remove the three lines that increment `self.sent` | `a_first_boot_sends_one_write_enable_per_write_and_the_application_no_write_disable` |
| 4 | `periph/mod.rs`: `Event::CacheInvalidated => self.extmem.invalidate(&self.mmu),` → `Event::CacheInvalidated => {}` | six tests: the five of this task and `the_stock_x4_ota_layout_boots_and_writes_otadata_on_its_first_boot_only`: the strict runs stop on a `StaleFlashWindow` |

The unit test of the counters: mutation 3 with `cargo test -p soc-esp32c3 --lib spi_mem` fails `every_command_sent_to_the_chip_is_counted_under_its_opcode_for_the_whole_run`.

Row 1 is why the fifth test exists: with the latch left set, the three lives still count, the bytes are still the measured ones and the hashes still match. Only the list of commands tells.

**Four mutations survive every test of this file**, and are held by unit tests alone. They were tried; all 12 tests pass under each:

| Mutation | Why Espressif's code cannot tell | The test that holds it |
|---|---|---|
| a program overwrites instead of AND-ing (`let programmed = *cell & byte;` → `let programmed = byte;`) | NVS and the bootloader only clear bits of erased or already-programmed words with values that are their AND | `flash::tests::a_program_only_clears_bits` (Task 3) |
| a sector erase erases nothing (`fill_erased` not called) | the only erase is the bootloader's, of an `otadata` sector that is already erased | `flash::tests::a_sector_erase_and_a_block_erase_set_their_whole_unit_and_nothing_else` (Task 3); on the real ROM, `the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next` (Task 10) |
| the third term of `page_version` removed | no measured firmware runs code it has just programmed | `machine::tests::code_decoded_from_the_flash_is_decoded_again_once_the_flash_is_programmed`, `…_erased` (Task 5) |
| an invalidation of a range drops nothing (only `(0, 0)` is honoured) | the invalidations that matter to these boots are of everything | `periph::extmem::tests::an_invalidation_drops_the_pages_of_the_whole_entries_it_covers_in_either_window` (Task 6) |

They are listed in the hand-over note: an erase issued by an application, the block erase, 64-byte programs and the split at a page boundary, `otadata` written by an application and the first NVS page change are covered by unit tests only.

- [ ] **Step 6: Format, lint, commit**

```bash
/usr/bin/git commit -m "test(soc): three lives of the NVS example: the counter survives, the bytes are the measured ones

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/soc-esp32c3 .github/workflows/ci.yml
```

---
## Task 13: The NVS reference firmware goes on counting in a second run on the same flash file

The second point of the milestone, through the binary, with a firmware nobody wrote for the test: a first `inkemu run` starts from `flash_nvs.bin` and keeps its flash in a file; a second one, given the file alone, is a power-on that prints `Restart counter = 1`; a third, long enough for a restart, counts `2` then `3`. Each run says the SHA-256 of what it started from and of what it saved, and they are the measured ones.

**Files:**
- Modify: `crates/emu-cli/tests/run_cli.rs` (one ignored test)
- Modify: `.github/workflows/ci.yml` (the filter of the reference tests, the count 2 → 3)

**Interfaces:**
- Consumes: `--flash-file` (Task 10); the summary lines of `RunFlash::finish` (Task 9) through `Counted::sha256(FLASH_AT_START)` and `Counted::sha256(FLASH_AT_END)`, `sha256_of`, `trace_hash_line` (helpers of `run_cli.rs`); `INKEMU_NVS_FLASH` (Task 11).
- Produces: the test `the_nvs_reference_firmware_goes_on_counting_in_a_second_run_on_the_same_flash_file`. In the CI, the tests of `run_cli` that need a reference image are now selected by the name fragment `reference_firmware`: the two `x4_boots_the_reference_firmware…` tests and this one. The job `rom-boot` skips the same fragment.

No code changes: the behaviour is that of Tasks 9 and 10. The test passes as soon as it is written; what proves it is the two mutations of Step 3.

- [ ] **Step 1: Write the test**

````diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -1689,3 +1689,102 @@ fn a_flash_file_whose_directory_became_read_only_exits_2_and_leaves_no_file() {
         .collect();
     assert_eq!(left, ["x4.bin.lock"], "no flash file, no temporary file");
 }
+
+/// `flash_nvs.bin` of `tools/reference-firmware/build.sh` and the whole flash after the first and
+/// the second life of its application (docs/research/esp-idf-storage-flash-traffic.md, §A.3, §B.2).
+const NVS_IMAGE_SHA256: &str = "c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f";
+const NVS_AFTER_LIFE_1: &str = "cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b";
+const NVS_AFTER_LIFE_2: &str = "7ad0acf62e5a4e20aa6d47be0f1d922a7b89cb204941feac1ab2fea71363b709";
+
+/// The milestone of the flash file through the real command, with a firmware nobody wrote for
+/// the test: ESP-IDF's NVS example keeps a restart counter, and a second `inkemu run` on the
+/// same flash file goes on counting where the first one stopped. One life of the example is
+/// 11.2 s of device time; its writes are done in the first second.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
+fn the_nvs_reference_firmware_goes_on_counting_in_a_second_run_on_the_same_flash_file() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let image =
+        std::path::PathBuf::from(std::env::var("INKEMU_NVS_FLASH").expect("set INKEMU_NVS_FLASH"));
+    assert_eq!(
+        sha256_of(&image),
+        NVS_IMAGE_SHA256,
+        "{} is not the pinned flash_nvs.bin; rebuild it with tools/reference-firmware/build.sh",
+        image.display()
+    );
+    let dir = tempfile::tempdir().unwrap();
+    let file = dir.path().join("x4.bin");
+    let run = |seconds: &str, flash: &[&std::ffi::OsStr]| {
+        let output = inkemu()
+            .args(["run", "--device", "x4", "--strict-mmio", "--trace-hash"])
+            .args(["--max-time", seconds, "--rom-dir", &rom_dir])
+            .args(flash)
+            .arg("--flash-file")
+            .arg(&file)
+            .timeout(std::time::Duration::from_secs(120))
+            .output()
+            .unwrap();
+        Counted {
+            code: output.status.code(),
+            console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
+            log: String::from_utf8_lossy(&output.stderr).into_owned(),
+        }
+    };
+
+    // First run: from the image, five seconds of the first life.
+    let first = run("5", &[std::ffi::OsStr::new("--flash"), image.as_os_str()]);
+    assert_eq!(first.code, Some(0), "{}", first.log);
+    assert!(
+        first.console.contains("rst:0x1 (POWERON)")
+            && first.console.contains(
+                "Reading restart counter from NVS ... The value is not initialized yet!\n"
+            )
+            && !first.console.contains("Restart counter = "),
+        "{}",
+        first.console
+    );
+    assert_eq!(first.sha256(FLASH_AT_START), NVS_IMAGE_SHA256);
+    assert_eq!(first.sha256(FLASH_AT_END), NVS_AFTER_LIFE_1);
+    assert_eq!(sha256_of(&file), NVS_AFTER_LIFE_1);
+    assert!(!first.log.contains("warning:"), "{}", first.log);
+
+    // Second run: the file alone. A power-on, and the counter the first run wrote.
+    let second = run("5", &[]);
+    assert_eq!(second.code, Some(0), "{}", second.log);
+    assert!(
+        second.console.contains("rst:0x1 (POWERON)")
+            && !second.console.contains("rst:0xc")
+            && second
+                .console
+                .contains("Reading restart counter from NVS ... Done\nRestart counter = 1\n")
+            && !second.console.contains("not initialized"),
+        "{}",
+        second.console
+    );
+    assert_eq!(second.sha256(FLASH_AT_START), NVS_AFTER_LIFE_1);
+    assert_eq!(second.sha256(FLASH_AT_END), NVS_AFTER_LIFE_2);
+    assert_eq!(sha256_of(&file), NVS_AFTER_LIFE_2);
+    assert!(!second.log.contains("warning:"), "{}", second.log);
+    // This firmware takes another path when it finds a counter: the two runs differ in their
+    // trace as in the flash they started from.
+    assert!(trace_hash_line(&first.log).is_some(), "{}", first.log);
+    assert_ne!(trace_hash_line(&first.log), trace_hash_line(&second.log));
+
+    // Third run: long enough for a restart. The counter goes on inside the run, then across it.
+    let third = run("12.5", &[]);
+    assert_eq!(third.code, Some(0), "{}", third.log);
+    let counted = third
+        .console
+        .find("Restart counter = 2\n")
+        .unwrap_or_else(|| panic!("no second count in:\n{}", third.console));
+    let after_the_restart = &third.console[counted..];
+    assert!(
+        after_the_restart.contains("rst:0xc (RTC_SW_CPU_RST)")
+            && after_the_restart.contains("Restart counter = 3\n"),
+        "{}",
+        third.console
+    );
+    assert_eq!(third.sha256(FLASH_AT_START), NVS_AFTER_LIFE_2);
+    assert_eq!(third.sha256(FLASH_AT_END), sha256_of(&file));
+    assert_eq!(sha256_of(&image), NVS_IMAGE_SHA256, "--flash is only read");
+}
````

- [ ] **Step 2: Run it; the CI job**

```bash
cargo test --workspace > /tmp/t13.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t13.log
tools/ci/run-ignored-tests.sh 3 --release -p emu-cli --test run_cli reference_firmware -- --ignored > /tmp/t13-i4.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 16 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware > /tmp/t13-i2.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` and `928` (an ignored test is not counted); `exit=0`; `exit=0`. The three reference tests of the CLI take 4.8 s in a release build on the host this plan was written on.

````diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -91,7 +91,7 @@ jobs:
         # un-ignored test would otherwise leave this job green with nothing proven.
         run: |
           tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored
-          tools/ci/run-ignored-tests.sh 16 --release -p emu-cli --test run_cli -- --ignored --skip x4_boots_the_reference_firmware
+          tools/ci/run-ignored-tests.sh 16 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware
   reference-boot:
     # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM
     # and runs it through a whole life: the application, its countdown, the restart it asks for
@@ -145,4 +145,4 @@ jobs:
         # Exact test counts, as in rom-boot: see tools/ci/run-ignored-tests.sh.
         run: |
           tools/ci/run-ignored-tests.sh 12 --release -p soc-esp32c3 --test reference_boot -- --ignored
-          tools/ci/run-ignored-tests.sh 2 --release -p emu-cli --test run_cli x4_boots_the_reference_firmware -- --ignored
+          tools/ci/run-ignored-tests.sh 3 --release -p emu-cli --test run_cli reference_firmware -- --ignored
````

- [ ] **Step 3: Mutations**

Each row: apply the change to `crates/emu-cli/src/flash_state.rs`, run `cargo test --release -p emu-cli --test run_cli reference_firmware -- --ignored > /tmp/m.log 2>&1; echo "exit=$?"`, see the test fail, undo.

| Mutation | What the test then sees |
|---|---|
| `Some(state) if from_file => Some(state.load()?),` → `Some(_) if from_file => seed,` (the content of the file is ignored: the run starts from an erased flash) | the second run finds no firmware: the ROM prints `invalid header: 0xffffffff` and no counter |
| `if self.from_file && modified == 0 {` → `if self.from_file {` (a run that started from the file never saves it) | the second run says `flash at end: not modified; … is left as it is` and no final SHA-256 |

- [ ] **Step 4: Format, lint, commit**

```bash
/usr/bin/git commit -m "test(cli): the NVS reference firmware goes on counting in a second run on the same flash file

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- crates/emu-cli/tests/run_cli.rs .github/workflows/ci.yml
```

---
## Task 14: The flash is written and kept in a file; hand-over of SP1b-3b

The documents for people, and the proof that nothing this plan did not do is still handed to it. The README says what the flash does now and what the flash file is (English, then French). The hand-over note (French) records the decisions, what the tests prove and what they do not, every new `ASSUMPTION:`, what goes to which plan, and what to carry into the spec. One test makes the table of registers refuse a plan that is done as an heir.

**Files:**
- Modify: `README.md`
- Modify: `crates/soc-esp32c3/tests/accepted_without_effect.rs` (one test)
- Create: `docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md` (French)

**Interfaces:**
- Consumes: `soc_esp32c3::periph::accepted_without_effect()` (each row has `peripheral`, `register`, `heir`).
- Produces: the test `every_row_hands_its_register_to_a_plan_that_is_still_to_come`, with the list `PLANS_TO_COME` (`SP1b-3d`, `SP1b-3f`, `SP1b-4`, `SP2`, `SP5a`, `SP5b`, `SP8`). The last task of each later plan removes its own name from that list, after it has handed on by name what it did not do.

- [ ] **Step 1: The test that refuses a plan that is done**

````diff
--- a/crates/soc-esp32c3/tests/accepted_without_effect.rs
+++ b/crates/soc-esp32c3/tests/accepted_without_effect.rs
@@ -33,3 +33,21 @@ fn the_documented_table_is_the_one_the_code_builds() {
     std::fs::write(&path, format!("{prose}{MARKER}\n{table}"))
         .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
 }
+
+/// The plans a row may hand its register to: those that are still to come. A plan that is done
+/// cannot be an heir: what it did not do is handed on by name before it ends, and its name is
+/// removed from this list by its last task.
+const PLANS_TO_COME: [&str; 7] = ["SP1b-3d", "SP1b-3f", "SP1b-4", "SP2", "SP5a", "SP5b", "SP8"];
+
+#[test]
+fn every_row_hands_its_register_to_a_plan_that_is_still_to_come() {
+    for row in accepted_without_effect() {
+        assert!(
+            PLANS_TO_COME.contains(&row.heir),
+            "{}.{} is handed to `{}`, which is no plan to come; name the plan that will give it its effect",
+            row.peripheral,
+            row.register,
+            row.heir
+        );
+    }
+}
````

```bash
cargo test -p soc-esp32c3 --test accepted_without_effect > /tmp/t14-a.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t14-a.log
```

Expected: `exit=0` and `2`. The test passes at once: Tasks 1 and 2 already renamed every heir. What proves it is the mutation: in `crates/soc-esp32c3/src/periph/extmem.rs`, change the heir of `EXTMEM.CACHE_MMU_OWNER` from `"SP1b-4"` to `"SP1b-3b"` and run the command again. Expected: `exit=101`; `every_row_hands_its_register_to_a_plan_that_is_still_to_come` fails ("EXTMEM.CACHE_MMU_OWNER is handed to `SP1b-3b`, which is no plan to come; name the plan that will give it its effect"), and so does `the_documented_table_is_the_one_the_code_builds`. Undo it.

- [ ] **Step 2: Nothing is still handed to this plan**

```bash
/usr/bin/git grep -n "SP1b-3b" -- crates > /tmp/t14-heirs.txt; wc -l < /tmp/t14-heirs.txt
grep -c "| SP1b-3b |" docs/registers-accepted-without-effect.md
for heir in SP1b-3d SP1b-3f SP1b-4 SP2 SP5a SP5b SP8; do echo "$heir $(grep -c "| $heir |" docs/registers-accepted-without-effect.md)"; done
/usr/bin/git grep -n "ASSUMPTION:" -- '*.rs' > /tmp/t14-assumptions.txt; wc -l < /tmp/t14-assumptions.txt
```

Expected: `0`; `0`; then `SP1b-3d 8`, `SP1b-3f 10`, `SP1b-4 28`, `SP2 26`, `SP5a 8`, `SP5b 2`, `SP8 22`; then `80` (77 before this plan: four added, by Tasks 2, 3, 4 and 6; one removed by Task 4, the address of a user transfer, which is now sourced). The four new ones are in the hand-over note, section « Hypothèses nouvelles »; check each against `/tmp/t14-assumptions.txt`.

- [ ] **Step 3: The README**

````diff
--- a/README.md
+++ b/README.md
@@ -2,7 +2,7 @@
 
 **A binary-level emulator for embedded e-ink devices.** inkemu runs the real, unmodified firmware image of a device on a virtual replica of its hardware. You can check how a firmware renders and behaves before you flash it.
 
-> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Flash writes and the device's flash file come next (sub-project 1b-3b), then a firmware given on its own (1b-3c), the serial consoles in both directions (1b-3d), a community firmware (1b-3e), deep sleep (1b-3f) and the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
+> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The flash is written as on the device: the bootloader records the OTA slot it chose, NVS keeps what the firmware stores, and `--flash-file` keeps the flash of a device from one run to the next. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Next come a firmware given on its own (sub-project 1b-3c), the serial consoles in both directions (1b-3d), a community firmware (1b-3e), deep sleep (1b-3f) and the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
 
 ## Build & try
 
@@ -58,6 +58,7 @@ inkemu: reset by the firmware, a CPU reset (rst:0xc) at pc 0x40048b92, sp 0x3fc8
 inkemu: cycle limit reached; raise --max-cycles to run longer
 inkemu: stopped at pc 0x4004c0d4 after 50000000 cycles (33222007 instructions)
 inkemu: 11459206 us of device time; CPU clock at 160 MHz
+inkemu: flash at start: sha256 fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2
 ```
 
 The second line is the restart the application asked for: inkemu reports every reset with its cause, the program counter and the stack pointer it found. With `--strict-mmio` the same run touches no register inkemu does not model and takes no CPU exception, across the restart.
@@ -69,8 +70,49 @@ inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3); timing model datasheet-uncalib
 inkemu: device-time limit reached; raise --max-time to run longer
 inkemu: stopped at pc 0x40381d1a after 27292718 cycles (18157194 instructions)
 inkemu: 5000000 us of device time; CPU clock at 160 MHz; the CPU is asleep, waiting for an interrupt
+inkemu: flash at start: sha256 fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2
 ```
 
+The last line names the flash the run started from: the whole chip, erased bytes included, so it is what `sha256sum` says of a 16 MiB image. It is an input of the run, as the ROM and the device are.
+
+### Keeping the flash between runs
+
+What a firmware writes to the flash (NVS settings, the OTA slot the bootloader chose, a file system) lives in the emulated chip, and is lost when the run ends unless `--flash-file` names a file to keep it in. The second reference image, ESP-IDF's `nvs_rw_value_cxx` example saved as `flash_nvs.bin`, keeps a restart counter in NVS:
+
+```text
+$ target/release/inkemu run --device x4 --flash flash_nvs.bin --flash-file x4.bin --max-time 5
+…
+Reading restart counter from NVS ... The value is not initialized yet!
+…
+inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3); timing model datasheet-uncalibrated (not calibrated); flash: flash_nvs.bin (16777216 bytes in 16 MiB), kept in x4.bin
+inkemu: device-time limit reached; raise --max-time to run longer
+inkemu: stopped at pc 0x40381d1a after 27896916 cycles (18606474 instructions)
+inkemu: 5000000 us of device time; CPU clock at 160 MHz; the CPU is asleep, waiting for an interrupt
+inkemu: flash at start: sha256 c3f0eed129d85de6c0f959d481bb99101d14d18ec6fab8bfd11c541128ec757f
+inkemu: flash at end: sha256 cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b; 1 of its 4 KiB sectors were modified; saved to x4.bin
+
+$ target/release/inkemu run --device x4 --flash-file x4.bin --max-time 5
+…
+rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
+…
+Reading restart counter from NVS ... Done
+Restart counter = 1
+…
+inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3); timing model datasheet-uncalibrated (not calibrated); flash: x4.bin (flash file, 16 MiB)
+…
+inkemu: flash at start: sha256 cfab4ffe5187eeea8ec2ee6e9794886289282f0242ce56aad2842144a6ffb17b
+inkemu: flash at end: sha256 7ad0acf62e5a4e20aa6d47be0f1d922a7b89cb204941feac1ab2fea71363b709; 1 of its 4 KiB sectors were modified; saved to x4.bin
+```
+
+The second run is a power-on of the device the first run left: it finds the counter. The rules of the flash file:
+
+- **It is the chip and nothing else**: a raw image of exactly the size of the flash of the device (16 MiB for the X4), with no header. `esptool.py` and `sha256sum` read it. A file of another size is refused with both sizes, and is never padded or cut; a symbolic link is not followed.
+- **Read at the start, written at the end.** A run reads the file when it exists and replaces it when it ends, whatever ends it: a limit, the first Ctrl-C, a `--strict-mmio` stop, a CPU that waits for ever, a reader that went away. A run that started from the file and modified nothing leaves it as it is. The file is replaced in one step (a temporary file in the same directory, then a rename), never written while the run goes on: a run that is killed, or ended by a second Ctrl-C, loses what it wrote to the flash and leaves the file of before.
+- **`--flash` is a seed, and is never written.** It is what a run starts from when the flash file does not exist yet. Given with a flash file that exists, it is refused before anything runs, because a run cannot start from two flashes: drop `--flash` to go on from the file, or add `--reset-flash` to start again from the image (or from an erased flash) and replace the file when the run ends.
+- **One run at a time.** A lock file beside the flash file (`x4.bin.lock`) is held for the whole run; a second `inkemu run` on the same file is refused at once, with the name of the lock. The lock file is left in place; it is empty, and the system frees the lock when the process ends, however it ends.
+- **A flash file that cannot be written when the run ends is an error**: the message says what could not be done and that the writes of the run are lost, and the exit code is 2 (a `--strict-mmio` stop keeps 3).
+- Without `--flash-file`, the summary says what was lost: `flash at end: 1 of its 4 KiB sectors were modified and are not kept, since the run has no flash file; pass --flash-file <path> to keep the flash of the device from one run to the next`.
+
 Things worth knowing:
 
 - **Two limits, two clocks.** `--max-cycles N` stops the run when the CPU has executed N cycles of its clock; `--max-time S` stops it when S seconds of device time have elapsed since power-on (a decimal number: `30`, `2.5`, `0.001`). `--max-cycles` counts the cycles the CPU executes, not device time: a sleeping CPU executes none while device time goes on, as it does across resets. A firmware that sleeps therefore reaches that limit late (the reference image waits on a timer for eleven seconds and still stops at `--max-cycles 50000000`), and one whose sleep nothing interrupts never does; `--max-time` bounds both. Both may be given; the first one reached ends the run, with exit code 0. The stop is exact and the same on every host: the first instruction boundary at or after the limit, or the limit itself when the CPU is asleep. A firmware that waits for an interrupt nothing can raise ends the run by itself, with exit code 4.
@@ -79,7 +121,8 @@ Things worth knowing:
 - **The console is as slow as the UART, and output only.** A byte leaves at the baud rate the firmware programmed; when a run stops, what was still in the UART is printed too. `--console` chooses which serial output of the device goes to standard output: `uart0` (the default), `uart1` or `usb-serial-jtag`; the ROM prints on the first and on the last. Nothing can be sent to the device yet: there is no UART reception and no host side of USB-Serial-JTAG (sub-project 1b-3d).
 - **Pipes and redirections.** `inkemu run … | head -5` ends when `head` has its lines: a run whose reader went away says so and ends with exit code 0. On Unix this also holds when the firmware prints nothing more: every so often, after a slice that had nothing to print, inkemu asks whether its reader is still there, and a reader that left is noticed after 64 such slices at most (a few milliseconds to half a second on the machine it was measured on, depending on what the firmware does). A run that ends by itself in the meantime ends as what it is, with the exit code of that end. On other systems such a silent run goes on to its limit or to Ctrl-C. When standard output cannot be written for another reason (a full disk), the run ends with exit code 2; a `--strict-mmio` stop keeps its code whatever happened to the console. A closed standard output (`>&-`) cannot be told from `/dev/null`, because the Rust runtime reopens it on the null device before the program starts: it behaves like `/dev/null`, the console is dropped and the run ends as it would have, with exit code 0.
 - **What is not emulated yet.** Deep sleep and light sleep: a firmware that asks for either keeps running. Sub-project 1b-3f models deep sleep and the wake-up by the RTC timer, and refuses light sleep by name. There is no radio, no display and no button yet. The first line of standard error says `timing model datasheet-uncalibrated (not calibrated)`: the cycle cost of each instruction comes from documents, not from a measurement on a device, so a device time is plausible, not exact (the calibration bench is sub-project 8).
-- **The flash is read-only.** On the first boot of a layout with OTA slots and a blank `otadata` partition, which is the layout of the Xteink X4, the bootloader records the slot it chose. inkemu reports the refused sector erase and page program (`flash command 0x20 is not modeled`, then `0x02`) and the boot goes on to the application; with `--strict-mmio` the run stops at the first of the two. Flash writes arrive with sub-project 1b-3b.
+- **The flash is written as a NOR chip is.** A program can only clear bits and wraps inside its 256-byte page; an erase sets a 4 KiB sector, a 64 KiB block or the whole chip to `0xFF`; both need the write-enable latch, which the chip clears when the operation completes. On the first boot of a layout with OTA slots and a blank `otadata` partition, which is the layout of the Xteink X4, the bootloader erases and programs `otadata`, and the next boots find it. Every operation completes at once: an erase that takes a tenth of a second on a device takes no time here (real durations come with the calibration bench, sub-project 8). What the chip model does not do is refused by name, never guessed: a command it does not have, a command sent with another address width or other dummy clocks than it knows, an address past the end of the chip, a program or an erase while a block-protect bit of the status register is set, a suspend request, several command bits in one write of `SPI1.CMD`. The status register is not part of the flash file: it starts from its factory value at every run.
+- **A read that may differ from the device is said.** inkemu reads the flash at every access through the cache windows; a device reads its cache, which keeps the bytes it loaded until the firmware invalidates it. When a firmware reads flash through a window, programs or erases it, and reads it again without invalidating the cache, inkemu returns the new bytes and warns that a device may still return the old ones (`read of the flash window at … may differ from the device`); `--strict-mmio` stops there. ESP-IDF invalidates what it writes, so its code never triggers this.
 - **Memory protection is not enforced.** The PMP entries, the permission registers and the stack guard are stored and never raise a fault (sub-project 1b-4).
 
 The options and the exit codes, as `inkemu run --help` prints them:
@@ -91,10 +134,12 @@ Usage: inkemu run [OPTIONS] --device <DEVICE>
 
 Options:
       --device <DEVICE>          Device profile id; an unknown id prints the available ones
-      --flash <FLASH>            Full flash image to boot: a dump of a device (`esptool.py read_flash 0 ALL flash.bin`), or the output of `esptool.py merge_bin`. Its bootloader, partition table and application run as on the device. Without it the flash is erased and the ROM reports an invalid header, as a blank chip does
+      --flash <FLASH>            Full flash image to boot: a dump of a device (`esptool.py read_flash 0 ALL flash.bin`), or the output of `esptool.py merge_bin`. Its bootloader, partition table and application run as on the device. Without it the flash is erased and the ROM reports an invalid header, as a blank chip does. The image is only read: what the firmware writes to the flash is kept by --flash-file, or lost when the run ends
+      --flash-file <FILE>        Keep the flash of the device in this file from one run to the next: a raw image of exactly the size of the flash. A run starts from the file when it exists (--flash is then refused unless --reset-flash is given) and writes it when it ends: at a limit, at the first Ctrl-C, at a --strict-mmio stop, when the CPU waits for ever. The file is replaced in one step, never written while the run goes on, so a run that is killed leaves the file of before. A lock file beside it (FILE.lock) refuses a second run on the same file; it is left in place
+      --reset-flash              Start from the --flash image, or from an erased flash, although the flash file exists, and replace the file when the run ends
       --max-cycles <MAX_CYCLES>  Stop when the CPU has executed this many cycles of its clock. The limit counts executed cycles, not device time: a sleeping CPU executes none, so a firmware that sleeps reaches it late, and one whose sleep nothing interrupts never does; --max-time bounds both
       --max-time <SECONDS>       Stop when this many seconds of device time have elapsed since power-on (a decimal number: 30, 2.5, 0.001). Device time goes on while the CPU sleeps and across resets
-      --strict-mmio              Stop at the first access to a register the emulator does not model, 8/16-bit write to a peripheral register, operation the emulator cannot perform faithfully, or access to flash that is not mapped (exit code 3)
+      --strict-mmio              Stop at the first access to a register the emulator does not model, 8/16-bit write to a peripheral register, operation the emulator cannot perform faithfully, access to flash that is not mapped, or read through a flash window that may differ from the device (exit code 3)
       --console <CONSOLE>        Which console of the device goes to standard output [default: uart0] [possible values: uart0, uart1, usb-serial-jtag]
       --rom-dir <ROM_DIR>        Use the ROM ELF files of this directory and never download
       --trace-hash               Print the execution trace hash at the end (determinism checks)
@@ -103,9 +148,12 @@ Options:
 Exit codes:
   0  the run ended normally: the cycle limit or the device-time limit was reached, Ctrl-C was
      pressed, or the reader of standard output went away
-  2  error: unknown device, missing or invalid ROM, refused flash image, or standard output cannot be written
+  2  error: unknown device, missing or invalid ROM, refused flash image or flash file, standard
+     output cannot be written, or the flash file could not be written when the run ended (what
+     the firmware wrote to the flash is then lost)
   3  stopped by --strict-mmio (an unmodelled register, an 8/16-bit register write, an operation the
-     emulator cannot perform faithfully, or flash that is not mapped)
+     emulator cannot perform faithfully, flash that is not mapped, or a read through a flash
+     window that may differ from the device)
   4  the CPU waits for an interrupt and none can come: no interrupt line is pending and no device
      has anything scheduled
 130  Ctrl-C was pressed a second time before the run had stopped: no summary was printed
@@ -114,12 +162,12 @@ Exit codes:
 The device console goes to standard output. Everything inkemu has to say goes to standard error:
 
 - first, the device, the timing model and the flash in use;
-- as the run goes, one `warning:` line for each thing it cannot do faithfully or that the firmware got wrong — an unmodelled register read or write (with the peripheral, the register, its address and the program counter), an 8- or 16-bit write to a peripheral register, an operation it cannot perform faithfully (a flash command other than identification and status: a program, an erase, a data read; an SPI flash transfer whose command is not 8 bits long; SHA through DMA; a SHA mode the chip does not have; a clock source it cannot turn into a frequency; an RTC slow clock other than the RC oscillator; a byte written to a UART whose transmit FIFO is full; a UART byte sent on a source clock it has no frequency for, such as the RC fast clock), an access to flash that no MMU entry maps, a CPU exception (named in words, with its program counter and stack pointer) — and one `inkemu:` line for each reset, which is an event of the run and not a warning;
-- at the end, why the run stopped, where (program counter, cycles, instructions), the device time elapsed and the CPU clock, followed by `; the CPU is asleep, waiting for an interrupt` when it is, then the most repeated lines with their count.
+- as the run goes, one `warning:` line for each thing it cannot do faithfully or that the firmware got wrong — an unmodelled register read or write (with the peripheral, the register, its address and the program counter), an 8- or 16-bit write to a peripheral register, an operation it cannot perform faithfully (a flash command the chip model does not have, or one it has, sent in a form or at an address it does not take; a program or an erase while a block-protect bit is set; a flash suspend request; several flash command bits written at once; an SPI flash transfer whose command is not 8 bits long; SHA through DMA; a SHA mode the chip does not have; a clock source it cannot turn into a frequency; an RTC slow clock other than the RC oscillator; a byte written to a UART whose transmit FIFO is full; a UART byte sent on a source clock it has no frequency for, such as the RC fast clock), an access to flash that no MMU entry maps, a read through a flash window that may differ from the device, a CPU exception (named in words, with its program counter and stack pointer) — and one `inkemu:` line for each reset, which is an event of the run and not a warning;
+- at the end, why the run stopped, where (program counter, cycles, instructions), the device time elapsed and the CPU clock, followed by `; the CPU is asleep, waiting for an interrupt` when it is, then the most repeated lines with their count, the trace hash when it was asked for, and the flash: the SHA-256 of the content the run started from, then what became of it (saved, with its SHA-256; left as it was; or modified and not kept).
 
 Each distinct warning and each distinct reset is listed once, however often it comes. `--strict-mmio` turns the first warning into a stop with exit code 3, for CI; CPU exceptions and resets are excepted, because they are what the firmware does on the device. Registers that inkemu stores without modelling their effect are listed in [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
 
-To build the reference image yourself (it needs Docker; nothing is executed on the host), see [tools/reference-firmware/README.md](tools/reference-firmware/README.md).
+To build the reference images yourself (it needs Docker; nothing is executed on the host), see [tools/reference-firmware/README.md](tools/reference-firmware/README.md).
 
 The first run downloads the ROM (4.9 MB) from Espressif's `esp-rom-elfs` release `20260528` and checks its SHA-256 before using it. To work offline, extract the archive yourself and pass `--rom-dir <directory>`. `inkemu run` reads only the ROM of its device's chip revision from that directory: `esp32c3_rev3_rom.elf` for `--device x4` and `--device x3`. `inkemu rom fetch --all [--rom-dir <directory>]` fills a directory in advance, with a single download, with the three ROMs of the supported chips (`esp32c3_rev0_rom.elf`, `esp32c3_rev3_rom.elf` and `esp32c3_rev101_rom.elf`); `inkemu rom fetch --device x4` fetches just the one that device needs.
 
@@ -151,7 +199,7 @@ Xteink devices are the first targets. The architecture is not tied to them, and
 
 ## Roadmap
 
-1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and the device's flash file, is next; then 1b-3c, a firmware given on its own (`--firmware`); 1b-3d, the serial consoles in both directions; 1b-3e, a community firmware boots; 1b-3f, deep sleep; 1b-4, the lockstep against QEMU.*
+1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and the device's flash file, done; 1b-3c, a firmware given on its own (`--firmware`), is next; then 1b-3d, the serial consoles in both directions; 1b-3e, a community firmware boots; 1b-3f, deep sleep; 1b-4, the lockstep against QEMU.*
 2. X4 peripherals, desktop UI, e-ink rendering: **first visual milestone**
 3. X3 support (UC8253 panel, I2C sensors)
 4. Debugging, resource monitor, headless test runner
@@ -168,6 +216,7 @@ Xteink devices are the first targets. The architecture is not tied to them, and
 - [Registers accepted without effect](docs/registers-accepted-without-effect.md): what is stored but not modelled, and the plan that makes each real
 - [ADR 0002](docs/adr/0002-flash-model-and-cache-windows.md): the flash model and the cache windows (French)
 - [ADR 0003](docs/adr/0003-contexte-des-peripheriques.md): the context every peripheral access receives, device time, and the three levels of reset (French)
+- [ADR 0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md): what an input of a run is, and the life of the flash file (French)
 
 ## Français
 
@@ -175,7 +224,7 @@ Cette partie reprend le document en français, section par section et en plus co
 
 **inkemu est un émulateur au niveau binaire d'appareils embarqués à écran e-ink.** Il exécute l'image réelle d'un firmware, sans la modifier, sur une réplique virtuelle du matériel : on peut vérifier ce qu'un firmware affiche et comment il se comporte avant de le flasher.
 
-**État : début du développement.** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète, la ROM charge le bootloader de second niveau d'ESP-IDF, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail. Les écritures en flash et le fichier de flash de l'appareil sont la prochaine étape (sous-projet 1b-3b) ; viennent ensuite le firmware fourni seul (1b-3c), les consoles série dans les deux sens (1b-3d), un firmware communautaire (1b-3e), le sommeil profond (1b-3f) et le lockstep avec QEMU (1b-4).
+**État : début du développement.** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète, la ROM charge le bootloader de second niveau d'ESP-IDF, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. La flash s'écrit comme sur l'appareil : le bootloader enregistre l'emplacement OTA qu'il a choisi, NVS garde ce que le firmware y range, et `--flash-file` conserve la flash d'un appareil d'une exécution à la suivante. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail. Viennent ensuite le firmware fourni seul (sous-projet 1b-3c), les consoles série dans les deux sens (1b-3d), un firmware communautaire (1b-3e), le sommeil profond (1b-3f) et le lockstep avec QEMU (1b-4).
 
 ### Construire et essayer
 
@@ -183,6 +232,15 @@ Cette partie reprend le document en français, section par section et en plus co
 
 L'image de référence (`hello_world` d'ESP-IDF v5.5.2) vit toute sa vie : `Hello world!`, onze secondes de compte à rebours, `esp_restart()`, puis un second démarrage que la ROM annonce par `rst:0xc (RTC_SW_CPU_RST)`. Les secondes d'attente ne sont pas exécutées : le CPU dort dans `wfi` et le temps de l'appareil saute au prochain événement de timer, si bien que les onze secondes prennent environ une seconde de temps d'hôte. Avec `--strict-mmio`, la même exécution ne touche aucun registre qu'inkemu ne modélise pas et ne prend aucune exception, redémarrage compris.
 
+**Conserver la flash d'une exécution à l'autre.** Ce qu'un firmware écrit en flash (réglages NVS, emplacement OTA choisi par le bootloader, système de fichiers) vit dans la puce émulée et se perd à la fin de l'exécution, sauf si `--flash-file <fichier>` nomme un fichier où le garder. La seconde image de référence (l'exemple `nvs_rw_value_cxx` d'ESP-IDF, `flash_nvs.bin`) tient un compteur de redémarrages en NVS : une première exécution avec `--flash flash_nvs.bin --flash-file x4.bin` affiche `The value is not initialized yet!`, une seconde avec `--flash-file x4.bin` seul démarre à froid et affiche `Restart counter = 1`. Les règles du fichier :
+
+- **C'est la puce et rien d'autre** : une image brute d'exactement la taille de la flash de l'appareil (16 Mio pour le X4), sans en-tête, lisible par `esptool.py` et par `sha256sum`. Un fichier d'une autre taille est refusé avec les deux tailles ; il n'est jamais complété ni coupé, et un lien symbolique n'est pas suivi.
+- **Lu au départ, écrit à la fin.** Une exécution lit le fichier s'il existe et le remplace quand elle se termine, quelle que soit sa fin : une limite, le premier Ctrl-C, un arrêt par `--strict-mmio`, un CPU qui attend pour toujours, un lecteur parti. Une exécution partie du fichier et qui n'a rien modifié le laisse tel quel. Le fichier est remplacé d'un coup (fichier temporaire du même répertoire, puis renommage), jamais écrit en cours d'exécution : une exécution tuée, ou terminée par un second Ctrl-C, perd ce qu'elle a écrit en flash et laisse le fichier d'avant.
+- **`--flash` est une graine, jamais écrite.** C'est ce dont part une exécution tant que le fichier de flash n'existe pas. Donnée avec un fichier de flash qui existe, elle est refusée avant toute exécution : retirer `--flash` pour continuer depuis le fichier, ou ajouter `--reset-flash` pour repartir de l'image (ou d'une flash effacée) et remplacer le fichier à la fin.
+- **Une exécution à la fois.** Un fichier de verrou voisin (`x4.bin.lock`) est tenu pendant toute l'exécution ; une seconde exécution sur le même fichier est refusée aussitôt. Le fichier de verrou reste en place ; il est vide, et le système libère le verrou à la fin du processus.
+- **Un fichier de flash impossible à écrire à la fin est une erreur** : le message dit ce qui n'a pas pu être fait et que les écritures de l'exécution sont perdues, et le code de sortie est 2 (un arrêt par `--strict-mmio` garde 3).
+- Sans `--flash-file`, le résumé dit ce qui a été perdu (`flash at end: 1 of its 4 KiB sectors were modified and are not kept…`). Dans tous les cas il nomme la flash de départ par son SHA-256 (`flash at start: sha256 …`) : c'est une entrée de l'exécution, comme la ROM et l'appareil.
+
 À savoir :
 
 - **Deux limites, deux horloges.** `--max-cycles N` arrête l'exécution quand le CPU a exécuté N cycles de son horloge ; `--max-time S` l'arrête quand S secondes du temps de l'appareil se sont écoulées depuis la mise sous tension (un nombre décimal : `30`, `2.5`, `0.001`). `--max-cycles` compte les cycles que le CPU exécute, non le temps de l'appareil : un CPU qui dort n'en exécute aucun alors que le temps de l'appareil continue, comme à travers les resets. Un firmware qui dort atteint donc cette limite tard (l'image de référence attend un timer pendant onze secondes et s'arrête tout de même à `--max-cycles 50000000`), et celui dont rien n'interrompt le sommeil ne l'atteint jamais ; `--max-time` borne les deux. Les deux peuvent être données ; la première atteinte termine l'exécution, avec le code de sortie 0. L'arrêt est exact et le même sur tout hôte : la première frontière d'instruction à la limite ou après, ou la limite elle-même quand le CPU dort. Un firmware qui attend une interruption que rien ne peut lever termine l'exécution de lui-même, avec le code 4.
@@ -191,11 +249,12 @@ L'image de référence (`hello_world` d'ESP-IDF v5.5.2) vit toute sa vie : `Hell
 - **La console est aussi lente que l'UART, et en sortie seulement.** Un octet part au débit que le firmware a programmé ; quand une exécution s'arrête, ce qui restait dans l'UART est affiché aussi. `--console` choisit quelle sortie série de l'appareil va sur la sortie standard : `uart0` (par défaut), `uart1` ou `usb-serial-jtag` ; la ROM écrit sur la première et sur la dernière. Rien ne peut encore être envoyé à l'appareil : ni réception UART, ni côté hôte de l'USB-Serial-JTAG (sous-projet 1b-3d).
 - **Tubes et redirections.** `inkemu run … | head -5` se termine quand `head` a ses lignes : une exécution dont le lecteur est parti le dit et finit avec le code 0. Sous Unix, c'est vrai aussi quand le firmware n'affiche plus rien : de temps en temps, après une tranche qui n'avait rien à afficher, inkemu demande si son lecteur est encore là, et un lecteur parti est remarqué après 64 de ces tranches au plus (de quelques millisecondes à une demi-seconde sur la machine où cela a été mesuré, selon ce que fait le firmware). Une exécution qui finit d'elle-même entre-temps finit comme telle, avec le code de sortie de cette fin. Ailleurs, une telle exécution muette continue jusqu'à sa limite ou jusqu'à Ctrl-C. Quand la sortie standard ne peut pas être écrite pour une autre raison (un disque plein), l'exécution finit avec le code 2 ; un arrêt par `--strict-mmio` garde son code quoi qu'il soit arrivé à la console. Une sortie standard fermée (`>&-`) ne se distingue pas de `/dev/null`, parce que l'exécutable Rust la rouvre sur le périphérique nul avant que le programme ne commence : elle se comporte comme `/dev/null`, la console est abandonnée et l'exécution finit comme elle l'aurait fait, avec le code 0.
 - **Ce qui n'est pas encore émulé.** Le sommeil profond et le sommeil léger : un firmware qui demande l'un ou l'autre continue de tourner. Le sous-projet 1b-3f modélise le sommeil profond et le réveil par le timer RTC, et refuse le sommeil léger en le nommant. Il n'y a encore ni radio, ni écran, ni bouton. La première ligne de l'erreur standard dit `timing model datasheet-uncalibrated (not calibrated)` : le coût en cycles de chaque instruction vient de documents, pas d'une mesure sur un appareil, donc un temps d'appareil est plausible, pas exact (le banc de calibration est le sous-projet 8).
-- **La flash est en lecture seule.** Au premier démarrage d'une disposition à emplacements OTA dont la partition `otadata` est vierge, celle du Xteink X4, le bootloader enregistre l'emplacement qu'il a choisi. inkemu signale l'effacement de secteur et la programmation de page refusés (`flash command 0x20 is not modeled`, puis `0x02`) et le démarrage continue jusqu'à l'application ; avec `--strict-mmio`, l'exécution s'arrête au premier des deux. Les écritures en flash arrivent avec le sous-projet 1b-3b.
+- **La flash s'écrit comme une puce NOR.** Une programmation ne peut qu'effacer des bits et se replie dans sa page de 256 octets ; un effacement met à `0xFF` un secteur de 4 Kio, un bloc de 64 Kio ou toute la puce ; les deux exigent le verrou d'écriture, que la puce fait retomber à la fin de l'opération. Au premier démarrage d'une disposition à emplacements OTA dont la partition `otadata` est vierge, celle du Xteink X4, le bootloader efface puis programme `otadata`, et les démarrages suivants la retrouvent. Chaque opération s'achève aussitôt : un effacement qui prend un dixième de seconde sur un appareil ne prend ici aucun temps (les durées réelles viennent avec le banc de calibration, sous-projet 8). Ce que le modèle de la puce ne fait pas est refusé en le nommant, jamais deviné : une commande qu'il n'a pas, une commande envoyée avec une autre largeur d'adresse ou d'autres cycles d'attente que ceux qu'il connaît, une adresse au-delà de la fin de la puce, une programmation ou un effacement quand un bit de protection de bloc du registre d'état est levé, une demande de suspension, plusieurs bits de commande dans une même écriture de `SPI1.CMD`. Le registre d'état n'est pas dans le fichier de flash : il repart de sa valeur d'usine à chaque exécution.
+- **Une lecture qui peut différer de l'appareil est dite.** inkemu lit la flash à chaque accès par les fenêtres du cache ; un appareil lit son cache, qui garde les octets qu'il a chargés jusqu'à ce que le firmware l'invalide. Quand un firmware lit de la flash par une fenêtre, la programme ou l'efface, puis la relit sans invalider le cache, inkemu renvoie les nouveaux octets et avertit qu'un appareil peut encore renvoyer les anciens (`read of the flash window at … may differ from the device`) ; `--strict-mmio` s'y arrête. ESP-IDF invalide ce qu'il écrit : son code ne déclenche jamais cet avertissement.
 - **La protection mémoire n'est pas appliquée.** Les entrées PMP, les registres de permission et le garde-pile sont mémorisés et ne lèvent jamais de faute (sous-projet 1b-4).
-- **Codes de sortie** : 0 quand l'exécution s'est terminée normalement (limite de cycles ou limite en temps de l'appareil atteinte, Ctrl-C, ou lecteur de la sortie standard parti) ; 2 pour une erreur (appareil inconnu, ROM absente ou invalide, image flash refusée, sortie standard impossible à écrire) ; 3 pour un arrêt par `--strict-mmio` ; 4 quand le CPU attend une interruption et qu'aucune ne peut venir ; 130 pour un second Ctrl-C.
+- **Codes de sortie** : 0 quand l'exécution s'est terminée normalement (limite de cycles ou limite en temps de l'appareil atteinte, Ctrl-C, ou lecteur de la sortie standard parti) ; 2 pour une erreur (appareil inconnu, ROM absente ou invalide, image flash ou fichier de flash refusé, sortie standard impossible à écrire, fichier de flash impossible à écrire à la fin de l'exécution) ; 3 pour un arrêt par `--strict-mmio` ; 4 quand le CPU attend une interruption et qu'aucune ne peut venir ; 130 pour un second Ctrl-C.
 
-La console de l'appareil va sur la sortie standard. Tout ce qu'inkemu a à dire va sur l'erreur standard : d'abord l'appareil, le modèle de temps et la flash utilisée ; au fil de l'exécution, une ligne `warning:` pour chaque chose qu'il ne sait pas faire fidèlement ou que le firmware a mal faite (lecture ou écriture d'un registre non modélisé, écriture de 8 ou 16 bits dans un registre de périphérique, opération qu'il ne peut pas exécuter fidèlement, accès à de la flash qu'aucune entrée MMU ne projette, exception CPU) et une ligne `inkemu:` pour chaque reset, qui est un événement de l'exécution et non un avertissement ; à la fin, la raison de l'arrêt, l'endroit, le temps d'appareil écoulé et l'horloge du CPU, puis les lignes les plus répétées. Chaque avertissement et chaque reset distinct n'est listé qu'une fois. `--strict-mmio` fait du premier avertissement un arrêt avec le code 3, pour la CI ; les exceptions CPU et les resets font exception, parce qu'ils sont ce que le firmware fait sur l'appareil. Les registres mémorisés sans que leur effet soit modélisé sont listés dans [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
+La console de l'appareil va sur la sortie standard. Tout ce qu'inkemu a à dire va sur l'erreur standard : d'abord l'appareil, le modèle de temps et la flash utilisée ; au fil de l'exécution, une ligne `warning:` pour chaque chose qu'il ne sait pas faire fidèlement ou que le firmware a mal faite (lecture ou écriture d'un registre non modélisé, écriture de 8 ou 16 bits dans un registre de périphérique, opération qu'il ne peut pas exécuter fidèlement, accès à de la flash qu'aucune entrée MMU ne projette, lecture par une fenêtre de flash qui peut différer de l'appareil, exception CPU) et une ligne `inkemu:` pour chaque reset, qui est un événement de l'exécution et non un avertissement ; à la fin, la raison de l'arrêt, l'endroit, le temps d'appareil écoulé et l'horloge du CPU, puis les lignes les plus répétées, l'empreinte de trace si elle est demandée, et la flash : le SHA-256 du contenu de départ, puis ce qu'il est devenu. Chaque avertissement et chaque reset distinct n'est listé qu'une fois. `--strict-mmio` fait du premier avertissement un arrêt avec le code 3, pour la CI ; les exceptions CPU et les resets font exception, parce qu'ils sont ce que le firmware fait sur l'appareil. Les registres mémorisés sans que leur effet soit modélisé sont listés dans [docs/registers-accepted-without-effect.md](docs/registers-accepted-without-effect.md).
 
 Pour construire soi-même l'image de référence (il faut Docker ; rien n'est exécuté sur l'hôte), voir [tools/reference-firmware/README.md](tools/reference-firmware/README.md). Le premier lancement télécharge la ROM (4,9 Mo) depuis la version `20260528` d'`esp-rom-elfs` d'Espressif et vérifie son SHA-256 avant de s'en servir ; pour travailler hors ligne, extraire l'archive soi-même et passer `--rom-dir <répertoire>`, ou remplir un répertoire à l'avance avec `inkemu rom fetch --all [--rom-dir <répertoire>]`. Autres commandes : `inkemu inspect <app.bin>` décrit une image d'application ESP-IDF et explique ce qui ne va pas quand il refuse un fichier ; `inkemu rvtest <elf>` exécute un test de conformité RISC-V sans système ([tools/conformance/README.md](tools/conformance/README.md)).
 
@@ -213,7 +272,7 @@ Espressif ESP32-C3 (RISC-V RV32IMC) : Xteink X4 et X3. Espressif ESP32-S3 (Xtens
 
 ### Feuille de route
 
-1. Cœur RV32IMC, ESP32-C3 minimal, chargeur de firmware, CLI : démarrer un firmware jusqu'à ses journaux de console. *1a fait ; 1b-1, squelette du SoC et démarrage de la ROM, fait ; 1b-2, flash et bootloader, fait ; 1b-3a, interruptions, timers, watchdogs et reset, fait ; 1b-3b, écritures en flash et fichier de flash de l'appareil, est le suivant ; puis 1b-3c, firmware fourni seul (`--firmware`) ; 1b-3d, consoles série dans les deux sens ; 1b-3e, démarrage d'un firmware communautaire ; 1b-3f, sommeil profond ; 1b-4, lockstep avec QEMU.*
+1. Cœur RV32IMC, ESP32-C3 minimal, chargeur de firmware, CLI : démarrer un firmware jusqu'à ses journaux de console. *1a fait ; 1b-1, squelette du SoC et démarrage de la ROM, fait ; 1b-2, flash et bootloader, fait ; 1b-3a, interruptions, timers, watchdogs et reset, fait ; 1b-3b, écritures en flash et fichier de flash de l'appareil, fait ; 1b-3c, firmware fourni seul (`--firmware`), est le suivant ; puis 1b-3d, consoles série dans les deux sens ; 1b-3e, démarrage d'un firmware communautaire ; 1b-3f, sommeil profond ; 1b-4, lockstep avec QEMU.*
 2. Périphériques du X4, interface de bureau, rendu e-ink : **premier jalon visuel**
 3. Prise en charge du X3 (dalle UC8253, capteurs I2C)
 4. Débogage, moniteur de ressources, exécution de tests sans écran
@@ -225,16 +284,16 @@ Espressif ESP32-C3 (RISC-V RV32IMC) : Xteink X4 et X3. Espressif ESP32-S3 (Xtens
 
 ### Documentation
 
-La [spécification](docs/superpowers/specs/2026-10-11-inkemu-design.md), les ADR [0002](docs/adr/0002-flash-model-and-cache-windows.md) (modèle de la flash et fenêtres de cache) et [0003](docs/adr/0003-contexte-des-peripheriques.md) (contexte des périphériques, temps de l'appareil et resets) sont en français ; la [recherche sur le matériel Xteink](docs/research/xteink-hardware.md) et la [table des registres acceptés sans effet](docs/registers-accepted-without-effect.md) sont en anglais.
+La [spécification](docs/superpowers/specs/2026-10-11-inkemu-design.md), les ADR [0002](docs/adr/0002-flash-model-and-cache-windows.md) (modèle de la flash et fenêtres de cache) [0003](docs/adr/0003-contexte-des-peripheriques.md) (contexte des périphériques, temps de l'appareil et resets) et [0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md) (entrées d'une exécution et fichier de flash) sont en français ; la [recherche sur le matériel Xteink](docs/research/xteink-hardware.md) et la [table des registres acceptés sans effet](docs/registers-accepted-without-effect.md) sont en anglais.
 
 ### Matériel tiers et licence
 
-La ROM masquée de l'ESP32-C3 est le code d'Espressif et n'est pas libre : inkemu ne la distribue jamais, il télécharge les fichiers ELF qu'Espressif publie sous Apache-2.0 dans `esp-rom-elfs` et les vérifie contre des empreintes épinglées. Le firmware de référence (bootloader ESP-IDF et exemple `hello_world`, Apache-2.0) est construit dans Docker et vérifié de même ; aucun binaire de firmware n'est stocké dans ce dépôt. `third_party/espressif-svd/` contient une copie non modifiée du `esp32c3.svd` d'Espressif (Apache-2.0), dont `crates/svd-regs` est généré. Voir l'[ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md). inkemu est sous [licence Apache 2.0](LICENSE). Xteink est une marque de son propriétaire ; ce projet n'est affilié ni à Xteink ni à Espressif.
+La ROM masquée de l'ESP32-C3 est le code d'Espressif et n'est pas libre : inkemu ne la distribue jamais, il télécharge les fichiers ELF qu'Espressif publie sous Apache-2.0 dans `esp-rom-elfs` et les vérifie contre des empreintes épinglées. Le firmware de référence (bootloader ESP-IDF, Apache-2.0, et deux exemples ESP-IDF non modifiés, `hello_world` et `nvs_rw_value_cxx`, dans le domaine public) est construit dans Docker et vérifié de même ; aucun binaire de firmware n'est stocké dans ce dépôt. `third_party/espressif-svd/` contient une copie non modifiée du `esp32c3.svd` d'Espressif (Apache-2.0), dont `crates/svd-regs` est généré. Voir l'[ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md). inkemu est sous [licence Apache 2.0](LICENSE). Xteink est une marque de son propriétaire ; ce projet n'est affilié ni à Xteink ni à Espressif.
 
 ## Third-party material
 
 - **Mask ROM.** The ESP32-C3 ROM is Espressif's code and is not open source. inkemu never ships it: it downloads the ROM ELF files that Espressif publishes under Apache-2.0 in [`esp-rom-elfs`](https://github.com/espressif/esp-rom-elfs) and verifies them against pinned hashes.
-- **Reference firmware.** The tests boot the stock ESP-IDF bootloader and `hello_world` example (Apache-2.0). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository.
+- **Reference firmware.** The tests boot the stock ESP-IDF bootloader (Apache-2.0) and two unmodified ESP-IDF examples, `hello_world` and `nvs_rw_value_cxx` (public domain, `Unlicense OR CC0-1.0`). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository.
 - **Register descriptions.** `third_party/espressif-svd/` holds an unmodified copy of Espressif's `esp32c3.svd` (Apache-2.0), from which `crates/svd-regs` is generated.
 
 See [ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md).
````

- [ ] **Step 4: The hand-over note**

Create `docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md`. Its section « Décisions prises pendant l'exécution » is to be filled with what the execution of this plan decided that the plan did not; its section « Coût en CI » holds the times measured on a development host, to be replaced by those of the first CI run on a hosted runner.

````markdown
# SP1b-3b — Note de passation

Plan : `docs/superpowers/plans/2026-10-12-sp1b-3b-flash-writes.md` (écritures en flash et fichier de flash de l'appareil). Décisions durables : ADR 0002, révisé (modèle de la flash et fenêtres de cache) et ADR 0004 (entrées d'un run et fichier de flash).

## Résultat

- **La flash s'écrit comme sur l'appareil.** La puce programme (ET bit à bit, repli dans la page de 256 octets), efface un secteur, un bloc de 64 Kio ou tout, et reçoit son registre d'état, sous le verrou d'écriture, qui retombe à la fin de chaque opération. SPI1 transmet l'adresse et les données de ses commandes câblées ; une écriture de `CMD` exécute une commande et une seule. Les six lectures de données répondent.
- **Un firmware compilé depuis ses sources garde un compteur en NVS.** L'exemple `storage/nvs/nvs_rw_value_cxx` d'ESP-IDF v5.5.2, non modifié, vit trois vies en mode strict sur ses deux images : `The value is not initialized yet!`, puis `Restart counter = 1`, puis `= 2`. Les onze programmations sont celles mesurées sous QEMU, dans l'ordre et octet pour octet ; la flash finit comme l'image mesurée (`fc23c7ec…2143f5`, et `6c668205…89de5b` derrière la table du X4).
- **La disposition OTA du X4 démarre en mode strict** : le bootloader efface puis programme `otadata` au premier démarrage, et à celui-là seulement ; les 32 octets sont ceux mesurés, l'effacement et la programmation sont comptés.
- **`inkemu run --flash-file <chemin>` conserve la flash d'un appareil d'un run au suivant.** Un second run sur le même fichier démarre à froid (`rst:0x1 (POWERON)`) et affiche le compteur que le premier a écrit ; le fichier vaut alors l'image mesurée après la deuxième vie (`7ad0acf6…363b709`). `--flash` n'est jamais écrite. Sans fichier, le résumé dit combien de secteurs ont été modifiés et perdus.
- **Déterminisme** : deux runs partis du même contenu ont le même hash de trace et laissent la même flash ; l'image `hello_world` garde son hash de trace `10174cc2726c3d77` (`--max-cycles 40000000`, mode strict).
- **Une lecture de fenêtre qui peut différer du silicium est signalée** (ADR 0002, point 6). Ni la ROM, ni le bootloader, ni ESP-IDF ne déclenchent ce diagnostic sur les images de référence.

## Ce que devient la note de SP1b-3a

Sa section « Reporté au plan SP1b-3b » a été partagée entre cinq plans par la première tâche (tableau « Réattribution du 12 octobre 2026 » en tête de cette note-là). Ce que ce plan en a pris est fait :

- écritures en flash, lectures de données par SPI1, adresse et données des commandes câblées, mise à jour d'`otadata` comptée dans le test OTA ;
- contenu rendu par la machine et état « modifié » (`Esp32C3::flash_contents`, `flash_modified_sectors`), fichier de flash, troisième terme de `Bus::page_version` ;
- le choix laissé ouvert par l'ADR 0002 : la vue en direct est gardée, et la lecture qui peut différer est nommée ;
- la règle de taille d'une `FlashDevice` venue de l'extérieur (note SP1b-2) : `SocBus::new` et `Peripherals::new` ne sont plus publics, une puce n'est construite que depuis `SocConfig::flash` ;
- le commentaire faux de `Spi1::run` et le test mal nommé (`USR | FLASH_PE` n'est pas l'encodage d'une programmation d'ESP-IDF : la programmation mesurée est `CMD = 0x02000000`, `ADDR = longueur << 24 | adresse`) ;
- l'hypothèse 13 de la note SP1b-2 (adresse d'un transfert utilisateur non décalée) : elle est sourcée pour 24 bits et n'est plus une hypothèse.

## Décisions prises pendant l'écriture du plan

Elles sont dans le tableau des décisions du plan, à partir de G19. Les principales :

- Plusieurs bits de commande dans une même écriture de `SPI1.CMD` sont refusés en les nommant, plutôt qu'exécutés dans un ordre deviné.
- SPI1 tient un journal borné (1 024 entrées, le surplus compté) de ce que la puce a écrit, et compte par opcode les commandes qu'il lui envoie : c'est ce que les tests comparent aux mesures.
- « Secteurs modifiés » et la génération du contenu ne comptent que les octets réellement changés : une programmation qui laisse chaque octet tel quel ne modifie rien, n'invalide rien et ne fait rien écrire.
- Les refus de la puce sont quatre diagnostics nommés (opcode inconnu, trame inconnue, hors de la puce, protégé) ; une écriture sans verrou est ignorée en silence, comme sur la pièce.
- Le fichier de flash est verrouillé par un fichier voisin `<chemin>.lock`, jamais supprimé.
- Un fichier de flash d'une autre taille est refusé même sous `--reset-flash` : une image de firmware donnée par erreur à `--flash-file` doit survivre.
- Un fichier de flash impossible à écrire à la fin est une erreur de code 2, sauf après un arrêt de `--strict-mmio`, qui garde 3 ; aucun nouveau code de sortie.
- Le SHA-256 du contenu de départ est imprimé à chaque run, avec ou sans fichier de flash (12 ms pour 16 Mio en version optimisée, mesuré).
- Les tests du firmware de référence sont sélectionnés par le fragment de nom `reference_firmware` dans la CI.
- Les deux exemples de documentation de `soc-esp32c3` (`SocBus::new`, `Esp32C3::flash_contents`) reçoivent leur `RomImage` en paramètre. `RomImage::from_flat` n'existe que sous la fonctionnalité `test-support` : un exemple qui l'appelait se compilait avec `cargo test --workspace`, où un autre crate active la fonctionnalité, et échouait avec `cargo test -p soc-esp32c3`. Pour l'exemple marqué `compile_fail`, c'était pire qu'un échec : il réussissait pour une mauvaise raison, quelle que soit la visibilité de `SocBus::new`. Un exemple `compile_fail` ne prouve que ce que sa mutation fait échouer.

## Décisions prises pendant l'exécution

Aucune : le plan a été exécuté tel qu'il est écrit.

## Ce que les tests prouvent, et ce qu'ils ne prouvent pas

Chaque comportement que le plan affirme nomme la mutation qui doit faire échouer son test ; elles ont été essayées. Quatre mutations survivent aux tests du firmware de référence, et sont tuées ailleurs :

- **ET ou écrasement.** NVS ne programme que des valeurs qui effacent des bits : le firmware de référence ne distingue pas une puce qui fait `ancien ET nouveau` d'une puce qui écrase. La règle est prouvée par le test unitaire `a_program_only_clears_bits`.
- **Un effacement qui n'efface rien.** Le secteur d'`otadata` que le bootloader efface est déjà effacé dans les images. L'effacement est prouvé par les tests unitaires de la puce et, à travers la commande, par le compteur de démarrages de `the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next`.
- **Le code décodé.** Aucun firmware de référence n'exécute du code qu'il vient de réécrire : le troisième terme de `page_version` est prouvé par des tests de la machine.
- **L'invalidation d'une plage.** Les images de référence ne dépendent que de l'invalidation de tout le cache (au démarrage, et par `bootloader_mmap`) ; l'invalidation par plage de `spi_flash_check_and_flush_cache` n'est couverte que par des tests unitaires.

Couverts par des tests unitaires seulement, faute d'un firmware de référence qui les exerce : un effacement demandé par une application, l'effacement de bloc `D8` (`CMD = 0x00800000`), les programmations de 64 octets et leur découpe à la frontière d'une page, `otadata` écrit par une application, le changement de page de NVS (il faut environ 125 vies par page).

Non testable : le vidage sur le disque avant le renommage du fichier de flash (`sync_all`). Les refus du script de construction du firmware quand une empreinte ne correspond pas n'ont pas été rejoués dans Docker ; l'étape de vérification de la CI (`sha256sum -c`) l'a été sur une copie altérée.

## Hypothèses nouvelles

Quatre marqueurs `ASSUMPTION:` s'ajoutent, un disparaît : `grep -rn "ASSUMPTION:" crates --include=*.rs` rend 80 lignes (77 à la fin de SP1b-3a), et les profils gardent leurs quatre marqueurs.

- `flash.rs`, `NorFlash::read_data` : les lectures quadruples (`6B`, `EB`) répondent quel que soit le bit QE du registre d'état. (Une image dont l'en-tête dit QIO ou QOUT, lue contre un appareil.)
- `flash.rs`, `NorFlash::write_status` : `SRWD` est mémorisé et ne verrouille rien. (Le niveau de la broche `WP#` : le modèle de broches de SP2, ou un schéma.)
- `periph/spi_mem.rs`, `Spi1::run_hard_wired` : la lecture câblée `FLASH_READ` envoie `ADDR[23:0]` et lit `MISO_DLEN` octets dans `W0..`. Ni la ROM ni ESP-IDF v5.5.2 n'écrivent ce bit. (Un firmware qui s'en sert, lu contre un appareil.)
- `periph/extmem.rs`, `Extmem::invalidate` : une invalidation de taille nulle à une adresse non nulle, ou à une adresse hors des deux fenêtres, ne retire rien. (Lire une fenêtre, la programmer, émettre une telle demande et relire, sur un appareil.)
- Retirée : l'adresse d'un transfert utilisateur prise non décalée dans `ADDR` (mesurée pour 24 bits : FW §A.1 et §P, SF §C).

## Reporté, par héritier

- **SP1b-3c** : `--firmware` (un `.bin` d'application seul), l'ADR sur le bootloader embarqué qui doit le précéder (il prend le numéro 0005), les tables de partitions dans les profils, les retouches de forme de `boards` (`# Errors` de `from_named_toml`, `ProfileError::Parse`).
- **SP1b-3d** : réception et interruptions UART, format de trame (8 lignes de la table), côté hôte de l'USB-Serial-JTAG, pilote de console commun aux tests d'intégration et leur accès à `test_support`.
- **SP1b-3e** : le démarrage d'un firmware communautaire (CrossPoint).
- **SP1b-3f** : sommeil profond et réveil par le timer RTC (10 lignes d'alimentation de la table), les bits 29:27 de `RTC_CNTL.TIME_UPDATE`, le découpage de `periph/timg.rs` et de `periph/rtc_cntl.rs` ; le sommeil léger y est refusé en le nommant.
- **SP1b-4** : un cache fidèle à la ligne (ADR 0002 : limites du diagnostic, et lecture après un changement d'entrée MMU sans invalidation) avec le coût d'un défaut ; l'enregistrement et le rejeu, qui portent les entrées d'un run (ADR 0004) ; les hypothèses ci-dessus ; les 28 lignes de la table qui le nomment.
- **SP2** : l'appareil virtuel doté d'une identité et l'endroit où ranger son fichier de flash ; une sauvegarde de la flash en cours de run si le besoin se confirme ; le niveau de `WP#` ; l'identifiant de `DeviceProfile` comme type validé.
- **SP8** : les durées réelles des opérations de flash (le bit d'occupation ne se lève jamais : le premier démarrage d'une disposition OTA est plus court ici que sur l'appareil d'une centaine de millisecondes), l'attente automatique et la suspension (`SPI1.FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL`, `SUS_STATUS` : la demande de suspension est refusée en la nommant), la carte de protection par blocs d'une pièce réelle (un programme ou un effacement sous un bit BP est refusé en le nommant). Le registre d'état n'est pas dans le fichier de flash : ses bits non volatils reviennent à leur valeur d'usine à chaque run.
- **SP9** : Windows. Les tests du verrou et du répertoire disparu supposent un système de fichiers POSIX ; `File::try_lock` existe sur Windows mais n'y a pas été essayé.
- **Aucun héritier** : les registres de programmation des eFuse restent des accès non modélisés, signalés par leur nom. Un fichier temporaire orphelin (`.<nom>.<pid>.tmp`) laissé par un processus tué pendant l'écriture finale n'est supprimé par aucun run.

## Lignes de la table des registres acceptés, par héritier

SP1b-3d : 8. SP1b-3f : 10. SP1b-4 : 28. SP2 : 26. SP5a : 8. SP5b : 2. SP8 : 22. Plus aucune ligne ne nomme SP1b-3b. Les trois lignes d'invalidation d'`EXTMEM` (`ICACHE_SYNC_CTRL`, `_ADDR`, `_SIZE`) disent désormais leur seul effet : mettre fin au diagnostic de lecture périmée des pages qu'elles couvrent.

## Coût en CI

Mesuré sur la machine de développement, pas sur un runner GitHub :

- construction du firmware de référence, seulement quand le cache est manqué : 34 s pour `hello_world` et 32 s de plus pour `nvs_rw_value_cxx` dans le même conteneur (le script imprime ces durées dans le journal du job) ;
- job `reference-boot` : `reference_boot` passe de 7 à 12 tests (2,7 s à 8,4 s), les tests de la commande de 2 à 3 (4,8 s) ;
- job `rom-boot` : les tests de la commande passent de 10 à 16 (2,1 s).

Le temps de compilation des jobs ne change pas : aucune dépendance n'est ajoutée.

## À reporter dans la spec

- **§5.1, persistance** : « chaque appareil virtuel possède son fichier flash » devient, dans la CLI, un fichier demandé par `--flash-file` ; sans l'option rien n'est conservé. Le fichier est écrit à la fin du run, pas au fil de l'eau : un run tué perd ses écritures. La « commande de remise à zéro explicite » est l'option `--reset-flash`. (ADR 0004.)
- **§5.1, entrées acceptées** : un dump complet donné à `--flash` n'est jamais écrit ; c'est le fichier de `--flash-file` qui porte l'état.
- **§3.1, déterminisme** : le contenu de la flash au départ est une entrée d'un run, nommée par son SHA-256 ; le hash de trace seul ne distingue pas deux runs partis de deux contenus quand le firmware suit le même chemin. (ADR 0004.)
- **§7, erreurs** : un fichier de flash impossible à écrire à la fin donne le code de sortie 2 (3 après un arrêt strict).
- **`--max-time`** : toujours absent de la spec (déjà signalé par la note SP1b-3a).
````

- [ ] **Step 5: Every gate, on the last commit**

```bash
cargo fmt --all --check > /tmp/t14-fmt.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/t14-clippy.log 2>&1; echo "exit=$?"
cargo test --workspace > /tmp/t14.log 2>&1; echo "exit=$?"; grep -c "^test .* ok$" /tmp/t14.log
cargo test -p soc-esp32c3 > /tmp/t14-soc.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/t14-i1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 16 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware > /tmp/t14-i2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 12 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/t14-i3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 3 --release -p emu-cli --test run_cli reference_firmware -- --ignored > /tmp/t14-i4.log 2>&1; echo "exit=$?"
cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 > /tmp/t14-wasm.log 2>&1; echo "exit=$?"
cargo deny check > /tmp/t14-deny.log 2>&1; echo "exit=$?"
cargo llvm-cov --workspace --fail-under-lines 80 > /tmp/t14-cov.log 2>&1; echo "exit=$?"; grep "^TOTAL" /tmp/t14-cov.log
target/release/inkemu run --device x4 --strict-mmio --max-cycles 40000000 --trace-hash --rom-dir "$INKEMU_ROM_ELF_DIR" --flash "$INKEMU_REF_FLASH" > /dev/null 2> /tmp/t14-hash.log; echo "exit=$?"; grep trace-hash /tmp/t14-hash.log
```

Expected: `exit=0` on every line; `929` tests; the four counts are those `.github/workflows/ci.yml` holds; line coverage 98.16 % in the scratch clone (the gate is 80 %); `trace-hash 10174cc2726c3d77`.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md
/usr/bin/git commit -m "docs: the flash is written and kept in a file; hand-over of SP1b-3b

Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ" -- README.md crates/soc-esp32c3/tests/accepted_without_effect.rs docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md
```
