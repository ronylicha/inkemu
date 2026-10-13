# SP1b-3c — `--firmware`: an application image boots behind the bootloader inkemu ships — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **As executed.** This plan is the record of what was intended; it is not rewritten. Its eleven tasks were followed by corrections, each with a test that fails before it and passes after, and with its mutations. The following changed after the tasks:
>
> - **A user's bootloader is judged on its segment addresses before its chip** (Task 4) → the chip is judged first, as the mask ROM does, and the speed code after it; the addresses come from one memory map per chip (`firmware::MEMORY_MAPS`) instead of ESP32-C3 constants, and a device whose chip has no map is refused by name (`UnmodelledChip`). A new chip family is a `SocFamily` variant in `crates/boards/src/lib.rs` (with its chip id and the SoC model it names), a `MEMORY_MAPS` entry, a `SHIPPED` entry and its directory under `third_party/esp-idf-bootloader/`.
> - **A file is named by the first check it fails** (Tasks 4 and 5) → an application longer than 0x8000 given as the bootloader is named an application; a dump whose table does not parse is named a whole flash image (`FlashImageWithoutTable`) when the file is long enough to hold the table (0x8000 + `PARTITION_TABLE_LEN`), and a bootloader followed by a few erased bytes is named too long for one (`BootloaderTooLong`); an image with no descriptor whose segments all load into RAM, given as the firmware, is named a bootloader; a damaged bootloader gets a bootloader's remedy; a bootloader followed by erased bytes is identified by the hash of its image, not of the file; the whole file, not only its image, must fit its slot.
> - **A partition table is judged against the flash** (Tasks 6 and 7) → also against what the bootloader boots and where inkemu writes the application: an empty name or one holding a NUL, two application rows of one subtype, a second `otadata`, an `otadata` that is not 0x2000 bytes, and (at profile load) a table with neither a factory nor an `ota_0` slot are refused. The subtype names and values are one public table of `firmware::partition` (`APP_SUBTYPES`, `DATA_SUBTYPES`, `subtype_value`, `subtype_name`), which the profile loader reads.
> - **A profile whose chip revision is below the minimum of its shipped bootloader is refused when a flash is composed** (Task 7) → it is refused when the profile is loaded (`ProfileError::ShippedNeedsLaterChip`), so that a front end never lists a bootloader that does not start the device; composition keeps its own check for a caller without a profile.
> - **The built-in profiles are listed in `BUILTIN`** (Task 7) → `crates/boards/build.rs` lists `crates/boards/profiles/*.toml`, sorted: a new device is one file and no code. Hidden, lock and backup files are left alone.
> - **`soc-esp32c3`, `firmware` and `boards` do no host I/O, with the WebAssembly build as the check** (Task 7) → it is refused by clippy, through a `clippy.toml` in each of those crates, and the CI clippy step (`tools/ci/clippy.sh`) also fails when a banned path names nothing; the WebAssembly step proves only that they build for the browser.
> - **The refusals of `flash_state.rs` change their wording only** (Task 9) → their remedies also depend on the option that gave the starting image, and `RunFlash::begin` passes that option to the refusal of a flash file of the wrong size (about 15 lines); a pipe given to `--flash`, `--firmware` or `--bootloader` is refused before it is opened; the warnings and the identity of a given bootloader are tested through the binary, and the warnings are printed only once the flash file accepts the composed flash; the help of `--flash-file` says that `--firmware`, like `--flash`, is refused next to a file that exists. Three rules stay in `emu-cli` for SP2 to move with `flash_state.rs`: the list of files a run only reads, the read limit of the application and the bootloader, and the bounded reader.
> - **`inkemu licenses` prints "the notices of what the binary carries"** (Task 8) → it prints the notices of the third-party material inkemu embeds as data, and says the notices of the Rust libraries it links are not listed yet (SP9); the bootloader's `NOTICE` states the rewrite of its header for a flash other than 16 MiB (Apache License 2.0, section 4(b)); the tests of `boards` pin each part's own notice. ADR 0005 says it refines spec §3.4, and §3.4 points to it.
> - **`ASSUMPTION:` lines**: 86 in `.rs` files at the base, 87 at the end of the branch (the DIO flash mode of every profile); in the profiles, 4 → 6 (the names, flags and MD5 entry of the X3 and X4 tables). Ignored tests: 18, 24, 13 and 8 → 18, 25, 13 and 8. `cargo test --workspace`: 1,111 passed, 65 ignored.
>
> The code blocks of the tasks are not the code as it stands on the branch, and the test counts, messages and document texts they state are not those of the tree. See the hand-over note, `docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md`, section « Décisions prises pendant l'exécution ».

**Goal:** a user who has an application image alone (a release of a community firmware, the `firmware.bin` of a PlatformIO build, the `.bin` of an ESP-IDF build) boots it as a freshly flashed device would. (1) `inkemu run --device x4 --firmware hello_world.bin`, with the reference `hello_world.bin` of `tools/reference-firmware` (the application alone), boots in `--strict-mmio` behind the ESP-IDF v5.5.2 bootloader inkemu ships and the stock partition table of the X4: the whole life, the restart and the second boot touch nothing unmodelled; the bootloader prints its 26 INFO lines, those of the QEMU capture of the same flash, including the table of the X4 and `Set actual ota_seq=1 in otadata[0]` on the first boot only. (2) With `nvs_rw_value_cxx.bin` and `--flash-file`, the restart counter goes on across two runs. (3) The composed flash is deterministic: the same files give the same bytes, whose SHA-256 is printed as `flash at start` and is that of the reference build merged under the X4 table (`9ddacb60…11bc89` for `hello_world`, `87832c6d…f28fec` for the NVS example); the trace hash of `--firmware hello_world.bin --strict-mmio --max-cycles 40000001` is pinned, `c6b24a1cc226ed4a`, and that of `--flash flash_hello.bin`, `10174cc2726c3d77`, does not move. (4) Every input that cannot be booted is refused before the run, cause then remedy, through the binary: not an image, an encrypted `.xota`, an ESP32-S3 image on an X4, an application larger than its slot, a corrupt checksum or digest, a whole dump or merged image (`--flash`), a bootloader given as the firmware (`--bootloader`), an application given as the bootloader (`--firmware`), a bootloader that requires a later chip than the device's. (5) `inkemu licenses` prints the notices of what the binary embeds, from a library a front end calls for its "About" page.

**Architecture:** everything a front end needs lives in two library crates that perform no host I/O and build for `wasm32-unknown-unknown` (the CI checks both from Task 7 on):

| Crate | Owns | Why a GUI can call it |
|---|---|---|
| `firmware` | the image readers (`FirmwareImage`, `AppDesc`, `BootloaderDesc`); the registry of shipped bootloaders (`SHIPPED`, bytes embedded with `include_bytes!`); the checks of a user's bootloader (`check_bootloader`) and of an application (`check_application`); the composition (`compose`: bytes in, bytes out); the `Device` they are judged against | pure functions on byte slices, typed errors (`BootloaderError`, `ApplicationError`, `ComposeError`) and typed results (`ComposedFlash`, `BootloaderIdentity`, `BootloaderWarning`) a front end matches on |
| `boards` | the device profiles: their partition table (`[[partition]]` rows, validated at load), the bootloader they select (`DeviceProfile::bootloader`, from the chip family), the device as the checks see it (`firmware_device`, with the eFuse block revision of the eFuse model), and `compose_flash(application, bootloader)`; the notices of the binary (`boards::notices`) | a front end picks a profile and two byte buffers; it shows the table, the bootloader's identity and the notices from the same values the CLI prints |

`emu-cli` parses `--firmware` and `--bootloader`, reads the two files with the bounded reader `--flash` already uses (`flash_file::read_regular_file`), calls `DeviceProfile::compose_flash`, names the file a refusal is about, and hands the composed bytes to the flash-file logic of SP1b-3b as the seed of the run (`flash_state::Seed`). That logic is a known debt handed to SP2 (it is host logic in the CLI crate); this plan changes the wording of three of its refusals (they name the option that gave the file) and adds no logic to it. The composed flash is: the shipped bootloader at offset 0 (its header set to the flash size of the device, as esptool does at offset 0) or the user's bootloader as it is; the partition table of the profile at `0x8000`; the application in the slot the bootloader starts on a blank `otadata` (the factory application, else `ota_0`); every other byte erased, `otadata` included; exactly the flash size of the device.

**Tech Stack:** Rust 1.99.0 (edition 2024); no new dependency (`boards` gains a dependency on the workspace crate `firmware`; `sha2`, `md-5`, `thiserror`, `serde`, `toml` are already there); `clap`, `assert_cmd`/`predicates`/`tempfile` (tests); the reference firmware tooling of `tools/reference-firmware` and the Docker image `espressif/idf:v5.5.2`, which this plan never runs (Task 3 copies one file out of it with `docker create`/`docker cp`, which starts no container).

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.3, §3.4 "bootloader embarqué, un par puce", §5.1 "Entrées acceptées", §5.2, §6.4, §7, §10.2 item 6, §10.3 criteria 2 and 7). Facts: `docs/research/shipped-bootloader-and-firmware-composition.md` (**SB**), `docs/research/flash-writes-and-crosspoint-boot.md` (**FW**: §H, §I, §J, §M, §N), `docs/research/sp1b-test-oracle.md` (**TO**: §2, §4), `docs/research/esp32c3-rom-flash-boot.md` (**RFB**: §F). ADR 0001 (how third-party Apache-2.0 material is shipped), ADR 0004 (the flash file), ADR 0005 (written by Task 9). Carried-over points: `docs/superpowers/notes/2026-10-12-sp1b-3b-carryover.md` (**N3b**). `IDF:` is a file of ESP-IDF v5.5.2.

Every code block of this plan was compiled and its tests run in a scratch clone of the repository, one commit per task, in the order of the tasks: after each task the workspace built, `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` were clean, `cargo test --workspace` passed and the WebAssembly check passed. The ignored tests were run against the real ROM and the reference images after Tasks 9, 10 and 11, with the counts the CI has. The numbers the steps state (test counts, hashes, console lines, error lines of the failing step) are the ones that clone produced. Each behaviour a task claims names the mutation that must make its test fail; each mutation in the tables was applied to the clone and the test named beside it failed. Applied in order to the commit before, the blocks of each task rebuild the commit of that task exactly: this was checked mechanically for the 11 tasks (with the two files Task 3 copies, checked by their SHA-256).

**How to read the code blocks.** A block headed `--- a/… +++ b/…` is a unified diff against the file as the previous step left it: apply it with the editor, hunk by hunk (the unmarked lines are context to find the place). "Create `path`:" introduces the whole content of a new file. "Create `path` with its tests only:" introduces the `#[cfg(test)] mod tests` of a new module, and "Put this above the tests of `path`:" the code that goes in front of it, in the same file. A step that says "the failing tests" gives test modules, test files and the module declarations they need; the step after it gives the code.


## Global Constraints

- Toolchain pinned: Rust **1.99.0**, edition **2024**. Workspace lints unchanged: `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `unsafe_code = "forbid"`; `unwrap_used`/`expect_used` denied outside tests (an invariant is documented with `// INVARIANT:`).
- No `TODO`, no placeholder, no `#[allow]` added to hide a lint.
- Coverage ≥ 80 % for the workspace (`cargo llvm-cov --workspace --fail-under-lines 80`); the image and composition code of `firmware` is held to its tests line by line (spec §10.3 criterion 8).
- Licence Apache-2.0; `cargo deny check` passes; **no dependency is added** (`boards` depends on the workspace crate `firmware`, which adds no package to `Cargo.lock`).
- Register addresses and names come from `svd-regs` (`svd_regs::esp32c3::addr("PERIPHERAL", "REGISTER")`), never from a literal.
- Every user-facing message states **the cause, then the remedy**, separated by a semicolon; each new error enum has a test that every variant does.
- **Untrusted input never panics and is bounded.** A user's firmware and bootloader files are untrusted, and so is a profile text. **No loop and no allocation is sized by a value a file or the firmware supplies without a constant bound** (segment counts, segment lengths, table entries, file sizes); each task that takes such a value says where its bound is.
- Determinism: same inputs → same composed bytes, same trace hash, same final flash. No wall clock, no host randomness, no `HashMap` iteration on the emulated path.
- `soc-esp32c3`, `firmware` and `boards` perform **no host I/O** and build for `wasm32-unknown-unknown` (`cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 -p firmware -p boards`, a step of the CI job `check` from Task 7 on).
- **GUI-first.** No logic in `emu-cli` beyond option parsing, file reading and writing, and printing: every refusal is a typed error of a library crate, which the CLI renders. Every option of this plan maps to a value a front end can show and set (`DeviceProfile::compose_flash(application, Option<bootloader>)`; the profile's `partitions` and `bootloader`; `boards::notices`).
- **The committed bootloader is the one third-party binary allowed in the repository**, under `third_party/` with its licence files; nothing else built or downloaded is committed. Third-party material is **never executed on the host**: inkemu is the only thing that executes a reference image.
- Each hardware or ESP-IDF behaviour a check relies on cites its source in a comment (report tag + section, or `IDF:` file and function). An assumed behaviour is marked `ASSUMPTION:` with what would settle it, and is listed in the hand-over note (Task 11).
- **A test proves only what a mutation makes fail.** Each task ends with a table of mutations: apply one, run the command, see the named test fail, undo it, and `touch` the file you restored (cargo judges a source by its modification time: a file restored with an older time leaves the mutant's build in place). Do not commit a task whose mutation survives.
- **No test bounded by wall-clock time when the work can be counted.** No test of this plan has a time bound of its own; the CLI tests keep `HANG_LIMIT` (15 minutes, measured margin in SP1b-3b).
- Code, tests, commit messages, the README, the ADR and the CLI never cite the decisions table of this plan, a review or a finding: they cite reports, ADRs, the spec and the code. Only the plan itself refers to the table (by its `C` numbers).
- Source files are changed with the editor (read, then edit the named lines), never by a script that rewrites them.
- Checking a command: redirect its output to a file, then read `$?`; never read an exit code through a pipe. Use `/usr/bin/git`. Run Python with `-I`.
- Commits: `type(scope): description`, only the paths of the task (`/usr/bin/git commit -- <paths>`, after `/usr/bin/git add` of the new files). **A commit message carries no trailer and no attribution line.**
- Formatting and lints before **every** commit: `cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"`, then `cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"` and `cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"`, all three `exit=0`, whether or not the task lists them. The code blocks of this plan are formatted as `rustfmt` leaves them.
- The ignored tests need `INKEMU_ROM_ELF_DIR` (the pinned esp-rom-elfs archive: `inkemu rom fetch --all --rom-dir <dir>`) and, for the reference images, `INKEMU_REF_FLASH`, `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH` (`flash_hello.bin`, `flash_nvs.bin` and `flash_nvs_ota.bin` of `tools/reference-firmware/build.sh`), as absolute paths. A task that says "ignored tests" runs `tools/ci/run-ignored-tests.sh` with the counts `.github/workflows/ci.yml` has at that point; a task that adds an ignored test updates the count in the same commit.
- **Docker is not run by this plan.** The reference images are those `tools/reference-firmware/build.sh` already produces (CI builds them in its `reference-boot` job when its cache is missed); Task 3 copies one file out of the image `espressif/idf:v5.5.2` without starting a container.
- Exit codes of `inkemu run` stay 0, 2, 3, 4 and 130. No task adds one.
- Documents for people (ADR 0005, the hand-over note) are in French, with all accents. README (English, then French), code, CLI output and this plan are in English.

## Review Focus

The input classes most likely to bite, each with the test that holds it and the mutation a final reviewer should try (apply, run the command of the task, see the test fail, undo, `touch` the file):

- **A dump or a merged image given to `--firmware`.** A partition table at `0x8000` is a flash image: "use it as the flash (`--flash`)" → `what_is_not_an_application_is_named_for_what_it_is` (Task 5), `every_input_that_cannot_be_booted_from_an_application_is_refused_before_boot` (Task 9). Mutation: remove `if crate::flash_image::has_partition_table(bytes) { return Err(ApplicationError::FlashImage); }` in `check_application`. A 16 MiB dump is read whole (it is the flash size, the read bound), then refused by the same check.
- **A bootloader given to `--firmware`, an application given to `--bootloader`.** The first carries `esp_bootloader_desc_t` (magic byte 80) at `0x20`, the second `esp_app_desc_t` (`0xABCD5432`); an application without a descriptor is still caught by its flash-mapped segments → `what_is_not_an_application_is_named_for_what_it_is` (Task 5), `an_application_given_as_the_bootloader_is_named_as_one` (Task 4), the CLI table (Task 9). Mutations: `if first.and_then(BootloaderDesc::parse).is_some() {` → `if false {`; delete the `image.app_desc.is_some()` check of `check_bootloader`; `FLASH_WINDOWS…any(…)` → `false`.
- **An ESP32-S3 image on a C3 profile.** Refused by chip id, both chips named → `an_application_for_another_chip_is_refused_with_both_chips` (Task 5), the CLI table (Task 9). Mutation: `if image.chip != device.chip {` → `if false {` in `check_application`.
- **An encrypted file.** `XOTA` is named; another first byte than `0xE9` names the likely cause (an encrypted or compressed update) → `what_is_not_an_application_is_named_for_what_it_is`, `a_whole_flash_image_an_ota_update_or_a_long_file_is_not_a_bootloader`.
- **An application larger than its slot.** The file length, not the image length, is compared (what is written is the file) → `an_application_larger_than_its_slot_is_refused` (exactly the slot passes). Mutation: `bytes.len() > slot.size as usize` → `bytes.len() > slot.size as usize + 1`.
- **A table that overflows a smaller flash.** Refused when the profile is loaded, and again by `compose` for a table given directly → `a_partition_table_that_does_not_fit_the_flash_is_refused_at_load` (Task 7), `a_table_that_does_not_fit_the_flash_or_starts_nothing_is_refused` (Task 6). Mutation: delete the `partitions.to_bytes(header_flash_size(flash)).map_err(…)?;` statement of `from_toml`.
- **A bootloader header claiming more flash than the profile.** Refused for a user's bootloader (the application would abort on the smaller chip); a smaller one is a warning; the shipped one is rewritten for the profile and its 4 MiB form is pinned → `a_header_the_rom_or_the_device_would_refuse_is_refused`, `a_smaller_flash_and_a_wrong_digest_are_warnings` (Task 4), `for_a_4_mib_flash_the_header_and_digest_are_those_of_the_4_mib_build` (Task 3). Mutations: `if header > flash {` → `>=`; delete the digest recomputation in `for_flash`.
- **A profile revision below the bootloader's minimum.** The shipped bootloader resets a v0.2 chip for ever: refused, with `--bootloader` as the remedy; a given bootloader that accepts it boots → `the_shipped_bootloader_is_refused_on_a_chip_it_would_reset_for_ever` (Task 6), `a_profile_composes_its_flash_with_its_bootloader_or_a_given_one` (Task 7); through the binary, a given bootloader that needs v0.4 → the CLI table (Task 9). Mutation: `if entry.min_chip_revision > device.chip_revision {` → `if false {`.
- **A truncated or huge file.** A file longer than the flash is refused on its metadata and never read; one that grows is cut one byte past the flash; a truncated image is refused by the image reader at the first segment that does not fit → `a_file_that_is_missing_too_long_or_no_file_is_refused_before_it_is_read` (Task 9, a sparse file of 16 MiB + 1). Mutation: `metadata.len() > u64::from(limit)` → `false` in `read_regular_file`: the test still sees `Grew`, not `TooLong`, and fails.
- **Segment counts and lengths that lie.** A count above 16 is refused before anything is allocated; a length larger than what remains is refused before it is copied; a bootloader segment the ROM would not load (unaligned, longer than 1 MiB, outside RAM, over the ROM's stack) is refused → `a_damaged_image_is_refused_with_what_is_wrong` (Task 5), `a_segment_the_rom_would_not_load_is_refused`, `a_header_the_rom_or_the_device_would_refuse_is_refused` (Task 4). Mutation: drop `&& !overlaps(ROM_RESERVED, addr, len)`.
- **What the reference firmware cannot tell.** The X4 and X3 profiles are both v0.3, the minimum of the shipped bootloader, and both 16 MiB: the revision refusal of the shipped bootloader and the header rewrite are held by unit tests only (a profile read from a text at v0.2; the measured 4 MiB build). A reviewer who weakens `the_shipped_bootloader_is_refused_on_a_chip_it_would_reset_for_ever` or `for_a_4_mib_flash_the_header_and_digest_are_those_of_the_4_mib_build` removes the only proof.

---

## Design decisions taken for this plan

C1–C15 were given to this plan and bind it. C16 onward were taken while writing it.

| # | Decision | Reason | Cost if wrong |
|---|---|---|---|
| C1 | **Ship the reference artefact**: the ESP-IDF v5.5.2 bootloader built by `tools/reference-firmware` (21,040 bytes, SHA-256 `601b48456b0a…87afaa3`, pinned in `SHA256SUMS.inputs`), INFO log level, no rollback, minimum chip revision v0.3, committed under `third_party/esp-idf-bootloader/` with `LICENSE` (ESP-IDF's Apache-2.0 text), `NOTICE` (Espressif's attribution and the BSD-3-Clause notices of `riscv/csr.h` and `riscv/encoding.h`), `PROVENANCE.md`, and a paragraph in the root `NOTICE`; a CI step proves the committed file is what the reference build produces. | SB §A, §D: the only artefact already pinned and tested, with a QEMU golden log; its INFO lines are the user's only view of the bootloader's choices. | Low. |
| C2 | **ADR 0005** (French): shipped, overridable, selected by profile; why this configuration; what a quiet second entry would cost and that it is a later, additive choice; the licence obligations; the S3 path. The spec is not edited. | The owner's decision; spec §3.4 already says "embarqué, un par puce". | None. |
| C3 | **Selection is data.** `firmware::SHIPPED` lists, per entry, its id, chip family, flash-mode family (DIO), ESP-IDF version, minimum chip revision, SHA-256 and bytes; a test holds every field to the bytes. A profile selects through `soc.family`; **the flash mode is not a profile key** (C19). A model of a known chip is a profile only; a new chip family is an entry and its directory. A test proves every bundled profile resolves to a bootloader. | SB §B: one binary per chip family and flash-mode family. | Low. |
| C4 | **Flash-size byte**: the shipped bootloader is written with the flash size of the profile in header byte 3's high nibble and its appended SHA-256 recomputed, as esptool does at offset 0; the 4 MiB form is pinned against the measured build (`d6421484b1ab…4f56109c5`). The application is never rewritten. A user's bootloader whose header claims more flash than the profile is refused (C17). | SB §B. | Low. |
| C5 | **Chip revision**: a profile below the shipped bootloader's minimum is refused before boot, with the remedy (`--bootloader` with a build that accepts it, or a profile at v0.3+). | It would reset for ever (SB §B). | None. |
| C6 | **`--bootloader <file>` overrides** the shipped one: read bounded, checked before boot (SB §G rows 1–11), shown in the summary (shipped id, a known hash, or its descriptor). inkemu never embeds or fetches a third-party bootloader. | The owner's decision; SB §G; FW §N. | Low. |
| C7 | **Partition tables in profiles**: `x4` states the stock X4 table (byte-identical to `tests/fixtures/partitions/x4-stock.bin`); `x3` states the layout of SB §F with the X4 names, `ASSUMPTION:` for names, flags and MD5 entry (compiled `334c9553…ae7d5e`). Validated against the flash at load. The application slot is derived, not a field (C16). `otadata` stays erased. | Spec §3.3, §5.1; N3b. | Low. |
| C8 | **Composition is a pure library function**: bytes in, bytes out, typed refusals; image exactly the flash size; no host I/O; wasm-buildable. | GUI-first. | None. |
| C9 | **Application checks before composing**: magic, `esp_app_desc_t`, chip id, chip-revision bounds against the profile, eFuse block-revision bounds against the eFuse model, checksum and digest, size ≤ slot, header flash ≤ device, not encrypted, not a dump, not a bootloader; each through the binary. The image reader of `firmware` is reused, not duplicated. | FW §M; spec §5.1, §10.3 criterion 7; SB §C. | Low. |
| C10 | **`--firmware` is a seed like `--flash`**: exclusive with `--flash`; refused next to an existing `--flash-file` unless `--reset-flash`; never written. Installing an application onto an existing flash file and keeping NVS goes to SP2. | ADR 0004. | Low. |
| C11 | **Notices reachable from the binary**: `inkemu licenses`, from `boards::notices`. | Apache-2.0 §4 once the bootloader bytes are in the binary (SB §A.3). | Low. |
| C12 | **Reference tests reuse what exists**: no new Docker run (C25); the QEMU golden log of the OTA layout (TO §2) is the oracle of the bootloader lines. | Continuity. | Low. |
| C13 | **CrossPoint is SP1b-3e's**: no CrossPoint test here; the SB §C observation goes to the hand-over note. | Scope; licence. | None. |
| C14 | **`inkemu inspect`** of a flash image with its bootloader's identity: handed to SP2 (C30). | It costs more than one task: `inspect` refuses whole images today. | Low. |
| C15 | **Not in this plan**, re-assigned by name in the hand-over note: a quiet bootloader entry, QIO/octal builds, the S3 bootloader and ROM, installing an application onto an existing flash file, moving `flash_state.rs` to a library crate. | One plan, one milestone. | None. |
| C16 | **The application goes to the factory application, else `ota_0`** — the reverse of C7's wording as given: that is what the bootloader starts on a blank or absent `otadata` (`IDF:bootloader_support/src/bootloader_utility.c`, `bootloader_utility_get_selected_boot_partition`: "Defaulting to factory image", else "No factory image, trying OTA 0"). A table with neither is refused (`ComposeError::NoApplicationSlot`). | Read in the bootloader's source. Neither profile has a factory slot: nothing changes for them. | None. |
| C17 | **The shipped bootloader is adapted to the flash size; a given one never is.** A given bootloader whose header says more flash than the device is refused, less is a warning. | A user's bootloader is what their build made; rewriting it would hide the mismatch. | Low. |
| C18 | **A digest that does not match on a given bootloader is a warning** (`BootloaderWarning::DigestMismatch`); its checksum is a refusal. | RFB §F step 7: the ROM boots it without secure boot; step 6: it refuses a checksum. | Low. |
| C19 | **No flash-mode key in profiles now**: every profile is DIO, a constant of `boards` (`FLASH_MODE`) used to select the registry entry. The first QIO or octal device adds the key with its build. | SB §B: every Xteink file seen is DIO; a QIO build is other code. | None. |
| C20 | **The table is `[[partition]]` rows of the profile**: `name`, `type` (`app`/`data`), `subtype` by its `gen_esp32part.py` name, `offset`, `size`, optional `encrypted`/`readonly`; required (an empty table is refused); compiled once at load with the writer of `firmware` and refused there with its origin. | Spec §3.3: a profile declares its tables; a GUI shows rows; the writer is byte-compatible with `gen_esp32part.py`. | Low. |
| C21 | **The eFuse block revision the application must accept comes from the eFuse model** (`Efuse::block_revision`, v1.2 from the production dump), not from a profile key. | One source of truth: the revision the bootloader reads at run time. | None. |
| C22 | **`firmware::Device`** carries chip, chip revision and eFuse block revision as `major * 100 + minor` (as headers and ESP-IDF count them) and the flash size as a header code; `firmware` stays free of `soc-esp32c3` types. | `firmware` depends on no workspace crate. | None. |
| C23 | **An application without a descriptor is refused**, and so is one whose header claims more flash than the device. | The bootloader reads the descriptor of every application; spec §5.1 refuses a declared flash larger than the profile's. | Low. |
| C24 | **The notices API is `boards::notices`** (`NOTICE`, `THIRD_PARTIES`, `notices_text()`), the command `inkemu licenses`. The licences of the Rust crates linked into the binary go to SP9. | `boards` ties devices together and depends on everything the binary carries; distribution packaging is SP9. | Low. |
| C25 | **The reference applications are cut out of the pinned merged images** at `0x10000` by the tests and checked against `SHA256SUMS.inputs`/`-nvs`; no new file, no new variable, no Docker run. | The merged images hold byte-identical copies (TO §2). | None. |
| C26 | **The CI proof of C1 is an ignored test of the `reference-boot` job**, `the_shipped_bootloader_is_the_one_the_reference_firmware_build_produced`, counted like the others. | A Rust test is counted by `run-ignored-tests.sh`; a shell step is not. | None. |
| C27 | **`LICENSE`, `NOTICE` and `PROVENANCE.md` sit in `third_party/esp-idf-bootloader/`, the binary in `esp32c3/`.** | One licence and one notice for every ESP-IDF bootloader; the S3 adds a file, a provenance row and its own notices. | None. |
| C28 | **Three refusals of `flash_state.rs` name the option that gave the file** (`Input::{Flash, Firmware, Bootloader}`), and `RunFlash::begin` takes a `Seed` with its option; the bounded file reader of `flash_file.rs` is shared with `firmware_file.rs`. Wording only: no logic is added to the module SP2 inherits. | A refusal that says `--flash` for a `--firmware` would send the user the wrong way. | Low. |
| C29 | **The refusal of an application given to `--flash` proposes `--firmware`.** | The remedy now exists. | None. |
| C30 | **`inkemu inspect` is left as it is**; describing a whole image or a flash file, with the identity of its bootloader, goes to SP2. | C14. | Low. |

## Carried-over points: where they are handled

| Point (source) | Task |
|---|---|
| `--firmware`, an application `.bin` alone (N3b « Reporté, par héritier ») | 9 |
| The ADR of the embedded bootloader, number 0005 (N3b; ADR 0004 « Numérotation ») | 9 |
| Partition tables in profiles (N3b) | 7 |
| The form fixes of `boards`: `# Errors` of `from_named_toml` omits `MissingFlash`; `ProfileError::Parse` repeats the remedy (N3b, from N2 and N3a) | 1 |
| The comment of `accepted_without_effect.rs` that lists SP1b-3c among the plans to come without a row | 11 |
| SB §G: the checks of a user's bootloader | 4 |
| SB §B, §C, §F; FW §M: the checks of an application and the composition | 5, 6 |
| Everything a document hands to "SP1b-3c" | 1 (checked: nothing this plan does not do), 11 (proved) |

## File Structure

```
.github/workflows/ci.yml                        WebAssembly check of firmware and boards (Task 7); reference-boot counts (Task 10)
NOTICE                                          the bootloader paragraph (Task 3)
README.md                                       booting an application image, licenses, roadmap, EN then FR (Task 11)
third_party/esp-idf-bootloader/                 new (Task 3)
  LICENSE, NOTICE, PROVENANCE.md
  esp32c3/bootloader.bin                        the reference bootloader, 21,040 bytes
docs/adr/0005-bootloader-embarque.md            new, in French (Task 9)
docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md   new, in French (Task 11)
tools/reference-firmware/README.md              the application images (Task 10)
crates/firmware/src/
  app_desc.rs        eFuse block revisions of the descriptor (Task 2)
  image.rs           FirmwareImage::len, parse_reporting_digest (Task 2)
  builder.rs         refresh_digest (Task 2); bootloader_desc_bytes, bootloader_spec (Task 4)
  chip.rs            FlashSize::nibble, from_bytes (Task 3)
  bootloader.rs      new: BootloaderDesc, ShippedBootloader, SHIPPED, shipped, for_flash (Task 3);
                     KNOWN, BootloaderIdentity, BootloaderWarning, BootloaderError, check_bootloader (Task 4)
  device.rs          new: Device, revision (Task 4)
  application.rs     new: ApplicationError, ApplicationReport, check_application (Task 5)
  compose.rs         new: Bootloader, ComposedFlash, ComposeError, compose (Task 6)
  partition.rs       PartitionTable::application_slot (Task 6); PartitionError: Clone (Task 7)
  error.rs           the refusal of an application given to --flash names --firmware (Task 11)
  lib.rs             modules and re-exports (Tasks 3 to 6)
crates/boards/
  Cargo.toml         firmware (Task 7)
  profiles/x4.toml, x3.toml   [[partition]] rows (Task 7)
  src/lib.rs         parse_detail (Task 1); partitions, bootloader, firmware_device, compose_flash,
                     SocFamily::chip_id (Task 7); the notices module (Task 8)
  src/notices.rs     new (Task 8)
crates/soc-esp32c3/
  src/periph/efuse.rs              Efuse::block_revision (Task 7)
  tests/accepted_without_effect.rs the plans to come without a row (Task 11)
crates/emu-cli/
  src/main.rs        SocFamily::chip_id, the new ProfileError variants (Task 7); licenses (Task 8);
                     --firmware, --bootloader, starting_flash (Task 9)
  src/flash_file.rs  ReadFailure, read_regular_file (Task 9)
  src/flash_state.rs Input, Seed; three refusals name the option (Task 9)
  src/firmware_file.rs new (Task 9)
  src/lib.rs         the module (Task 9)
  tests/licenses_cli.rs new (Task 8)
  tests/run_cli.rs   --firmware and --bootloader through the binary (Task 9); the reference
                     firmware from its application images (Task 10); --flash names --firmware (Task 11)
```

---

## Task 1: What is handed to SP1b-3c, and the shape of `boards`

The hand-over note of SP1b-3b hands four points to SP1b-3c: `--firmware`, the ADR of the embedded bootloader, partition tables in profiles, and two form fixes of `boards` that earlier notes left to whoever touched the crate. This plan does all four; nothing is re-assigned. This task checks that no document hands anything else to SP1b-3c, then makes the two form fixes: the `# Errors` section of `from_named_toml` names every error it can return, and a refused profile states one remedy instead of two (a refusal of this crate's deserializers carries its own; the TOML parser's do not, and get the general one).

**Files:**
- Modify: `crates/boards/src/lib.rs`

**Interfaces:**
- Consumes: `boards::ProfileError::Parse`, `DeviceProfile::from_toml`.
- Produces: `fn parse_detail(error: &toml::de::Error) -> String` (private); `ProfileError::Parse` displays `"{origin}: {detail}"`, `detail` ending with one remedy.

**Bounds on untrusted input.** None new: `parse_detail` reads the last line of a message the TOML parser built.

- [ ] **Step 1: Check what names SP1b-3c**

```bash
/usr/bin/git grep -n "SP1b-3c" -- . ':!docs/superpowers/plans/*' ':!docs/research/*' > /tmp/t1-3c.txt; echo "exit=$?"; wc -l < /tmp/t1-3c.txt
```

Expected: `exit=0` and `5`: `crates/soc-esp32c3/tests/accepted_without_effect.rs:40` (SP1b-3c owns no row of the table yet; Task 11 drops it from that comment), ADR 0004 line 50 (the ADR number this plan uses), the re-assignment table of the SP1b-3a note, and lines 159 and 171 of the SP1b-3b note: the « Reporté » list, which names the four points above, and the sentence that says SP1b-3c enters `PLANS_TO_COME` with its first row of the table (this plan adds none). The research report written for this plan names it in its scope line; it is a dated document. Nothing is to be re-assigned.

- [ ] **Step 2: Write the failing tests**

````diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -374,6 +374,28 @@
         DeviceProfile::from_toml("demo.toml", text)
             .unwrap_err()
             .to_string()
+    }
+
+    #[test]
+    fn a_refused_profile_states_one_remedy() {
+        // A refusal of this crate carries its own remedy, and no other is added after it.
+        let size = error_for(&VALID.replace("size_mib = 4", "size_mib = 3"));
+        assert!(
+            size.ends_with("; write size_mib as 1, 2, 4, 8 or 16"),
+            "{size}"
+        );
+        assert!(!size.contains("correct the device profile"), "{size}");
+        // A refusal of the parser has none, and gets the general one, once.
+        let key = error_for(&format!("{VALID}x = 1\n"));
+        assert!(
+            key.contains("unknown field `x`") && key.ends_with("; correct the device profile"),
+            "{key}"
+        );
+        assert_eq!(
+            key.matches("; correct the device profile").count(),
+            1,
+            "{key}"
+        );
     }
 
     #[test]
````


- [ ] **Step 3: Run them and see them fail**

```bash
cargo test -p boards --lib > /tmp/t1-red.log 2>&1; echo "exit=$?"; grep "FAILED\|^test result" /tmp/t1-red.log
```

Expected: `exit=101`; `test tests::a_refused_profile_states_one_remedy ... FAILED` and `19 passed; 1 failed`: the size refusal ends with `; correct the device profile` after its own remedy.

- [ ] **Step 4: The remedy is added only where none is**

````diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -49,7 +49,8 @@
 
 #[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
 pub enum ProfileError {
-    #[error("{origin}: {detail}; correct the device profile")]
+    /// `detail` ends with a remedy: its own, or the general one ([`parse_detail`]).
+    #[error("{origin}: {detail}")]
     Parse { origin: String, detail: String },
     #[error(
         "{origin}: the profile has no [flash] table; add one with its two keys: size_mib ({sizes}) and jedec_id (0xMMTTCC, the three bytes the chip answers to a read-id command)",
@@ -226,6 +227,23 @@
         .map_err(serde::de::Error::custom)
 }
 
+/// What a refusal of the TOML says, with one remedy. The deserializers of this crate end their
+/// message with a remedy of their own (`…; write size_mib as …`); the parser's messages (an unknown
+/// key, a missing one, broken TOML) have none and get the general one. The message is the last
+/// line: the lines above it show where in the file the problem is.
+fn parse_detail(error: &toml::de::Error) -> String {
+    let detail = error.to_string().trim_end().to_owned();
+    let has_remedy = detail
+        .lines()
+        .next_back()
+        .is_some_and(|message| message.contains("; "));
+    if has_remedy {
+        detail
+    } else {
+        format!("{detail}; correct the device profile")
+    }
+}
+
 #[must_use]
 pub fn builtin_ids() -> &'static [&'static str] {
     &BUILTIN_IDS
@@ -251,7 +269,7 @@
     pub fn from_toml(origin: &str, text: &str) -> Result<DeviceProfile, ProfileError> {
         let raw: RawProfile = toml::from_str(text).map_err(|e| ProfileError::Parse {
             origin: origin.to_owned(),
-            detail: e.to_string().trim_end().to_owned(),
+            detail: parse_detail(&e),
         })?;
         let RawFlash(flash) = raw.flash.ok_or_else(|| ProfileError::MissingFlash {
             origin: origin.to_owned(),
@@ -268,7 +286,8 @@
     /// Like [`DeviceProfile::from_toml`], and checks that the profile carries the id it is loaded under.
     ///
     /// # Errors
-    /// [`ProfileError::Parse`], or [`ProfileError::IdMismatch`] when the ids differ.
+    /// [`ProfileError::Parse`] or [`ProfileError::MissingFlash`] as [`DeviceProfile::from_toml`]
+    /// says, or [`ProfileError::IdMismatch`] when the ids differ.
     pub fn from_named_toml(
         expected_id: &str,
         origin: &str,
````


- [ ] **Step 5: Run the tests**

```bash
cargo test -p boards --lib > /tmp/t1.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t1.log
cargo test --workspace > /tmp/t1-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t1-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `20 passed`; then `exit=0` and `1006 60` (tests passed and ignored in the workspace).

- [ ] **Step 6: Mutations**

Each row: apply the change, run `cargo test -p boards --lib`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `crates/boards/src/lib.rs`: `if has_remedy {` → `if false {` | `a_refused_profile_states_one_remedy` |
| 2 | `if has_remedy {` → `if true {` | `a_refused_profile_states_one_remedy` |

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "fix(boards): a refused profile states one remedy; from_named_toml lists every error" -- crates/boards/src/lib.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 2: An image knows its length and its descriptor's eFuse block revisions; a digest can be reported

Three facts the checks of Tasks 4 and 5 need from the image reader, without a second parser: the length of an image (its trailer included: what follows in a file is not part of it), the eFuse block revisions an application accepts (`min_efuse_blk_rev_full` and `max_efuse_blk_rev_full` of `esp_app_desc_t`, bytes 176 and 178, `IDF:esp_app_format/include/esp_app_desc.h`), and a parse that reports an appended SHA-256 that does not match instead of refusing it (the mask ROM boots such a bootloader without secure boot: RFB §F step 7). The test builder gains `refresh_digest`, so that a test can change a header byte (the digest covers the header) and keep the image valid.

**Files:**
- Modify: `crates/firmware/src/app_desc.rs`
- Modify: `crates/firmware/src/builder.rs`
- Modify: `crates/firmware/src/image.rs`

**Interfaces:**
- Consumes: `FirmwareImage::parse`, `AppDesc::parse`, `builder::{ImageSpec, build_image, app_desc_bytes}`.
- Produces: `pub len: usize` on `FirmwareImage`; `pub min_efuse_block_revision: u16` and `pub max_efuse_block_revision: u16` on `AppDesc`; `pub fn FirmwareImage::parse_reporting_digest(bytes: &[u8]) -> Result<(FirmwareImage, bool), FirmwareError>` (`true` when the digest matches or none is appended; a checksum is still required); `pub fn builder::refresh_digest(image: &mut [u8])` (test support).

**Bounds on untrusted input.** Unchanged: the segment count is at most 16 (`MAX_SEGMENTS`) before anything is allocated, each segment length is checked against what remains of the slice before it is copied, and the digest is read only when 32 bytes are there.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/firmware/src/app_desc.rs
+++ b/crates/firmware/src/app_desc.rs
@@ -56,3 +56,33 @@
         })
     }
 }
+#[cfg(test)]
+mod tests {
+    use super::*;
+    use crate::builder::app_desc_bytes;
+
+    #[test]
+    fn the_efuse_block_revisions_are_read_where_esp_idf_puts_them() {
+        let mut desc = app_desc_bytes("1", "demo", "v5.5.2");
+        desc[176..178].copy_from_slice(&3u16.to_le_bytes());
+        desc[178..180].copy_from_slice(&199u16.to_le_bytes());
+        let parsed = AppDesc::parse(&desc).unwrap();
+        assert_eq!(
+            (
+                parsed.min_efuse_block_revision,
+                parsed.max_efuse_block_revision
+            ),
+            (3, 199)
+        );
+        assert_eq!(parsed.project_name, "demo");
+    }
+
+    #[test]
+    fn a_segment_too_short_or_without_the_magic_holds_no_descriptor() {
+        let desc = app_desc_bytes("1", "demo", "v5.5.2");
+        assert!(AppDesc::parse(&desc[..LEN - 1]).is_none());
+        let mut other = desc.clone();
+        other[0] ^= 1;
+        assert!(AppDesc::parse(&other).is_none());
+    }
+}
````


````diff
--- a/crates/firmware/src/builder.rs
+++ b/crates/firmware/src/builder.rs
@@ -141,6 +141,19 @@
     }
 
     #[test]
+    fn a_header_byte_changed_and_the_digest_refreshed_is_a_valid_image() {
+        use crate::image::FirmwareImage;
+        let mut image = build_image(&ImageSpec::default());
+        image[15..17].copy_from_slice(&4u16.to_le_bytes());
+        assert!(
+            FirmwareImage::parse(&image).is_err(),
+            "the digest covers the header"
+        );
+        refresh_digest(&mut image);
+        assert_eq!(FirmwareImage::parse(&image).unwrap().min_chip_rev_full, 4);
+    }
+
+    #[test]
     #[should_panic(expected = "would overlap the partition table")]
     fn an_image_that_reaches_the_partition_table_is_refused() {
         let spec = ImageSpec {
````


````diff
--- a/crates/firmware/src/image.rs
+++ b/crates/firmware/src/image.rs
@@ -237,6 +237,51 @@
     }
 
     #[test]
+    fn the_length_of_an_image_ends_with_its_trailer_whatever_follows_it() {
+        for hash_appended in [true, false] {
+            let spec = ImageSpec {
+                hash_appended,
+                ..ImageSpec::default()
+            };
+            let bytes = build_image(&spec);
+            let mut longer = bytes.clone();
+            longer.extend([0xFF; 100]);
+            assert_eq!(FirmwareImage::parse(&bytes).unwrap().len, bytes.len());
+            assert_eq!(FirmwareImage::parse(&longer).unwrap().len, bytes.len());
+        }
+    }
+
+    #[test]
+    fn a_digest_that_does_not_match_can_be_reported_instead_of_refused() {
+        let mut bytes = build_image(&ImageSpec::default());
+        assert_eq!(
+            FirmwareImage::parse_reporting_digest(&bytes).map(|(_, matches)| matches),
+            Ok(true)
+        );
+        let last = bytes.len() - 1;
+        bytes[last] ^= 0xFF;
+        assert_eq!(
+            FirmwareImage::parse(&bytes),
+            Err(FirmwareError::HashMismatch)
+        );
+        let (image, matches) = FirmwareImage::parse_reporting_digest(&bytes).unwrap();
+        assert!(!matches);
+        assert_eq!(image.len, bytes.len());
+        // The checksum is never only reported.
+        let spec = ImageSpec {
+            hash_appended: false,
+            ..ImageSpec::default()
+        };
+        let mut bytes = build_image(&spec);
+        let last = bytes.len() - 1;
+        bytes[last] ^= 0xFF;
+        assert!(matches!(
+            FirmwareImage::parse_reporting_digest(&bytes),
+            Err(FirmwareError::ChecksumMismatch { .. })
+        ));
+    }
+
+    #[test]
     fn accepts_image_without_hash() {
         let spec = ImageSpec {
             hash_appended: false,
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p firmware --lib > /tmp/t2-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t2-red.log
```

Expected: `exit=101`; the crate does not compile: ``no field `min_efuse_block_revision` on type `app_desc::AppDesc` ``, ``no field `max_efuse_block_revision` …``, ``cannot find function `refresh_digest` in this scope``.

- [ ] **Step 3: The length, the two revisions, the reported digest**

````diff
--- a/crates/firmware/src/app_desc.rs
+++ b/crates/firmware/src/app_desc.rs
@@ -14,6 +14,9 @@
 const IDF_VER: (usize, usize) = (112, 32);
 const ELF_SHA256: usize = 144;
 const SHA256_LEN: usize = 32;
+/// `min_efuse_blk_rev_full` and `max_efuse_blk_rev_full` (`esp_app_desc.h` of ESP-IDF v5.5).
+const MIN_EFUSE_BLOCK_REVISION: usize = 176;
+const MAX_EFUSE_BLOCK_REVISION: usize = 178;
 
 #[derive(Clone, Debug, PartialEq, Eq)]
 pub struct AppDesc {
@@ -24,12 +27,20 @@
     pub date: String,
     pub idf_ver: String,
     pub elf_sha256: [u8; 32],
+    /// The eFuse block revisions the application accepts, `major * 100 + minor`; 0 and 65535 mean
+    /// "no bound", as the bootloader reads them (`bootloader_common_loader.c`, `IS_FIELD_SET`).
+    pub min_efuse_block_revision: u16,
+    pub max_efuse_block_revision: u16,
 }
 
 fn c_string(bytes: &[u8], (at, len): (usize, usize)) -> String {
     let field = &bytes[at..at + len];
     let end = field.iter().position(|&b| b == 0).unwrap_or(len);
     String::from_utf8_lossy(&field[..end]).into_owned()
+}
+
+fn le_u16(b: &[u8], at: usize) -> u16 {
+    u16::from_le_bytes([b[at], b[at + 1]])
 }
 
 fn le_u32(b: &[u8], at: usize) -> u32 {
@@ -53,9 +64,12 @@
             date: c_string(segment, DATE),
             idf_ver: c_string(segment, IDF_VER),
             elf_sha256,
+            min_efuse_block_revision: le_u16(segment, MIN_EFUSE_BLOCK_REVISION),
+            max_efuse_block_revision: le_u16(segment, MAX_EFUSE_BLOCK_REVISION),
         })
     }
 }
+
 #[cfg(test)]
 mod tests {
     use super::*;
````


````diff
--- a/crates/firmware/src/builder.rs
+++ b/crates/firmware/src/builder.rs
@@ -71,6 +71,17 @@
         out.extend(digest);
     }
     out
+}
+
+/// Recomputes the SHA-256 appended to `image`, over every byte before it: a test that changes a
+/// header byte (the digest covers the header) keeps the image valid with this.
+///
+/// # Panics
+/// When `image` is shorter than a digest.
+pub fn refresh_digest(image: &mut [u8]) {
+    let hashed = image.len() - 32;
+    let digest = Sha256::digest(&image[..hashed]);
+    image[hashed..].copy_from_slice(&digest);
 }
 
 /// An erased flash byte, which fills the gap between the image and the partition table.
````


````diff
--- a/crates/firmware/src/image.rs
+++ b/crates/firmware/src/image.rs
@@ -44,6 +44,9 @@
     pub segments: Vec<Segment>,
     pub hash_appended: bool,
     pub app_desc: Option<AppDesc>,
+    /// Bytes of the image, from its header to the end of its trailer (the checksum, and the
+    /// digest when one is appended). What follows in a file is not part of the image.
+    pub len: usize,
 }
 
 pub(crate) fn le_u16(b: &[u8], at: usize) -> u16 {
@@ -60,6 +63,26 @@
     /// # Errors
     /// Any [`FirmwareError`] describing why the file is not a usable application image.
     pub fn parse(bytes: &[u8]) -> Result<FirmwareImage, FirmwareError> {
+        Self::parse_checked(bytes, true).map(|(image, _)| image)
+    }
+
+    /// Like [`FirmwareImage::parse`], but an appended SHA-256 that does not match is reported
+    /// (`false`) instead of refused: the mask ROM boots a bootloader whose digest is wrong when
+    /// secure boot is off (`docs/research/esp32c3-rom-flash-boot.md`, §F step 7). The checksum is
+    /// still required: the ROM refuses an image without it.
+    ///
+    /// # Errors
+    /// Any [`FirmwareError`] of [`FirmwareImage::parse`] but [`FirmwareError::HashMismatch`].
+    pub fn parse_reporting_digest(bytes: &[u8]) -> Result<(FirmwareImage, bool), FirmwareError> {
+        Self::parse_checked(bytes, false)
+    }
+
+    /// Parses an image; the answer says whether its appended digest matches (`true` when none
+    /// is appended). A digest that does not match is an error when `require_digest` is set.
+    fn parse_checked(
+        bytes: &[u8],
+        require_digest: bool,
+    ) -> Result<(FirmwareImage, bool), FirmwareError> {
         if bytes.starts_with(XOTA_MAGIC) {
             return Err(FirmwareError::EncryptedOta);
         }
@@ -89,9 +112,12 @@
             .ok_or(FirmwareError::BadFlashSize { raw: size_code })?;
         let (segments, checksum, end) = Self::read_segments(bytes, usize::from(count))?;
         let hash_appended = bytes[HASH_APPENDED_OFFSET] != 0;
-        Self::verify_trailer(bytes, end, checksum, hash_appended)?;
+        let (len, digest_matches) = Self::verify_trailer(bytes, end, checksum, hash_appended)?;
+        if require_digest && !digest_matches {
+            return Err(FirmwareError::HashMismatch);
+        }
         let app_desc = segments.first().and_then(|s| AppDesc::parse(&s.data));
-        Ok(FirmwareImage {
+        let image = FirmwareImage {
             chip: ChipId::from_raw(le_u16(bytes, 12)),
             entry: le_u32(bytes, 4),
             spi_mode,
@@ -102,7 +128,9 @@
             segments,
             hash_appended,
             app_desc,
-        })
+            len,
+        };
+        Ok((image, digest_matches))
     }
 
     /// Reads `count` segments; returns them, the XOR checksum of their data, and the
@@ -146,14 +174,15 @@
         Ok((segments, checksum, pos))
     }
 
-    /// Checks the checksum byte (at the next `position % 16 == 15`) and, when announced,
-    /// the SHA-256 digest that follows it.
+    /// Checks the checksum byte (at the next `position % 16 == 15`) and, when announced, compares
+    /// the SHA-256 digest that follows it. Returns the length of the image and whether its digest
+    /// matches (`true` when none is appended).
     fn verify_trailer(
         bytes: &[u8],
         end: usize,
         checksum: u8,
         hash_appended: bool,
-    ) -> Result<(), FirmwareError> {
+    ) -> Result<(usize, bool), FirmwareError> {
         let checksum_pos = end + (TRAILER_ALIGN - 1 - end % TRAILER_ALIGN);
         let stored = *bytes
             .get(checksum_pos)
@@ -164,16 +193,15 @@
                 computed: checksum,
             });
         }
-        if hash_appended {
-            let hashed_len = checksum_pos + 1;
-            let stored_hash = bytes
-                .get(hashed_len..hashed_len + SHA256_LEN)
-                .ok_or(FirmwareError::TruncatedTrailer)?;
-            if Sha256::digest(&bytes[..hashed_len]).as_slice() != stored_hash {
-                return Err(FirmwareError::HashMismatch);
-            }
-        }
-        Ok(())
+        let hashed_len = checksum_pos + 1;
+        if !hash_appended {
+            return Ok((hashed_len, true));
+        }
+        let stored_hash = bytes
+            .get(hashed_len..hashed_len + SHA256_LEN)
+            .ok_or(FirmwareError::TruncatedTrailer)?;
+        let matches = Sha256::digest(&bytes[..hashed_len]).as_slice() == stored_hash;
+        Ok((hashed_len + SHA256_LEN, matches))
     }
 
     /// # Errors
````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p firmware --lib > /tmp/t2.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t2.log
cargo test --workspace > /tmp/t2-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t2-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `59 passed`; then `exit=0` and `1011 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test -p firmware --lib`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `image.rs`: `if require_digest && !digest_matches {` → `if false {` | `a_digest_that_does_not_match_can_be_reported_instead_of_refused`, `rejects_bad_checksum_and_bad_hash`, `a_header_byte_changed_and_the_digest_refreshed_is_a_valid_image` |
| 2 | `image.rs`: `Ok((hashed_len + SHA256_LEN, matches))` → `Ok((hashed_len, matches))` | `the_length_of_an_image_ends_with_its_trailer_whatever_follows_it`, `a_digest_that_does_not_match_can_be_reported_instead_of_refused` |
| 3 | `app_desc.rs`: `const MIN_EFUSE_BLOCK_REVISION: usize = 176;` → `178` | `the_efuse_block_revisions_are_read_where_esp_idf_puts_them` |
| 4 | `builder.rs`: delete `image[hashed..].copy_from_slice(&digest);` | `a_header_byte_changed_and_the_digest_refreshed_is_a_valid_image` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(firmware): an image knows its length and its descriptor's eFuse block revisions; a digest can be reported" -- crates/firmware/src/app_desc.rs crates/firmware/src/builder.rs crates/firmware/src/image.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 3: inkemu ships the ESP-IDF v5.5.2 bootloader of the ESP32-C3, with its licence and provenance

The bootloader of the reference build is committed, with what Apache-2.0 §4 and the two BSD-3-Clause notices ask a redistribution to carry (SB §A.3), and embedded in the binary by a registry of `firmware`: one entry per chip family and flash-mode family, each field held by a test to the bytes it describes. The entry writes itself for a flash of any size: the size nibble of its header and its appended digest, as esptool does at offset 0 (SB §B); the 4 MiB form is the measured 4 MiB build, byte for byte.

**Files:**
- Modify: `NOTICE`
- Create: `crates/firmware/src/bootloader.rs`
- Modify: `crates/firmware/src/chip.rs`
- Modify: `crates/firmware/src/lib.rs`
- Create: `third_party/esp-idf-bootloader/LICENSE`
- Create: `third_party/esp-idf-bootloader/NOTICE`
- Create: `third_party/esp-idf-bootloader/PROVENANCE.md`
- Create: `third_party/esp-idf-bootloader/esp32c3/bootloader.bin`

**Interfaces:**
- Consumes: `FirmwareImage::parse`, `FirmwareImage::len`, `ChipId`, `SpiMode`, `FlashSize`.
- Produces: `firmware::bootloader::{BOOTLOADER_DESC_MAGIC, BootloaderDesc { version, idf_ver, date_time }, BootloaderDesc::parse(&[u8]) -> Option<_>, sha256_hex(&[u8]) -> String, ShippedBootloader { id, chip, flash_mode, idf_version, min_chip_revision, sha256, bytes }, SHIPPED: [ShippedBootloader; 1], shipped(ChipId, SpiMode) -> Option<&'static ShippedBootloader>, ShippedBootloader::for_flash(&self, FlashSize) -> Vec<u8>}`; `FlashSize::nibble(self) -> u8`, `FlashSize::from_bytes(u64) -> Option<FlashSize>`; re-exports `BootloaderDesc`, `SHIPPED`, `ShippedBootloader`.

**Bounds on untrusted input.** `BootloaderDesc::parse` reads 80 bytes of a slice it checks to be that long. `for_flash` works on the shipped bytes only.

- [ ] **Step 1: The bootloader and its licence, from the reference build and the pinned image**

The binary is the first 21,040 bytes of `flash_hello.bin` (the reference build merges its bootloader there unchanged, TO §2); the licence is ESP-IDF's own, copied out of the pinned image without starting a container. Both are checked by their SHA-256 before anything else:

```bash
mkdir -p third_party/esp-idf-bootloader/esp32c3
head -c 21040 "$INKEMU_REF_FLASH" > third_party/esp-idf-bootloader/esp32c3/bootloader.bin; echo "exit=$?"
docker create --name inkemu-license espressif/idf:v5.5.2 > /tmp/t3-create.log 2>&1; echo "exit=$?"
docker cp inkemu-license:/opt/esp/idf/LICENSE third_party/esp-idf-bootloader/LICENSE > /tmp/t3-cp.log 2>&1; echo "exit=$?"
docker rm inkemu-license > /tmp/t3-rm.log 2>&1; echo "exit=$?"
sha256sum third_party/esp-idf-bootloader/esp32c3/bootloader.bin third_party/esp-idf-bootloader/LICENSE > /tmp/t3-sums.txt; echo "exit=$?"; cat /tmp/t3-sums.txt
```

Expected: five `exit=0`, then `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3` for the bootloader (the value of `tools/reference-firmware/SHA256SUMS.inputs`) and `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` for the licence (11,358 bytes: the Apache License 2.0 as ESP-IDF v5.5.2 ships it, at `$IDF_PATH/LICENSE` in the pinned image). Any other value: stop; the image or the reference build is not the pinned one.

Then the notice, the provenance and the paragraph of the root `NOTICE` (the two BSD-3-Clause texts are those of `components/riscv/include/riscv/csr.h` and `encoding.h` in the same image, comment markers removed):

Create `third_party/esp-idf-bootloader/NOTICE`:

````text
ESP-IDF 2nd-stage bootloader (esp32c3/bootloader.bin)
Copyright Espressif Systems (Shanghai) Co., Ltd.
Licensed under the Apache License, Version 2.0 (see LICENSE in this directory).
Source: https://github.com/espressif/esp-idf, tag v5.5.2 (commit 30aaf64524299d3bde422ca9a2848090d1bc5d0f),
built from unmodified sources; only the build configuration is inkemu's (see PROVENANCE.md).

The binary contains code expanded from two headers of ESP-IDF, distributed under the following
terms.

components/riscv/include/riscv/csr.h:

Copyright (c) 2013, The Regents of the University of California (Regents).
Copyright (c) 2018-2019, The libfemto authors
Copyright (c) 2020 Espressif Systems (Shanghai) PTE LTD
All Rights Reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.
3. Neither the name of the Regents nor the
   names of its contributors may be used to endorse or promote products
   derived from this software without specific prior written permission.

IN NO EVENT SHALL REGENTS BE LIABLE TO ANY PARTY FOR DIRECT, INDIRECT,
SPECIAL, INCIDENTAL, OR CONSEQUENTIAL DAMAGES, INCLUDING LOST PROFITS, ARISING
OUT OF THE USE OF THIS SOFTWARE AND ITS DOCUMENTATION, EVEN IF REGENTS HAS
BEEN ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

REGENTS SPECIFICALLY DISCLAIMS ANY WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
PURPOSE. THE SOFTWARE AND ACCOMPANYING DOCUMENTATION, IF ANY, PROVIDED
HEREUNDER IS PROVIDED "AS IS". REGENTS HAS NO OBLIGATION TO PROVIDE
MAINTENANCE, SUPPORT, UPDATES, ENHANCEMENTS, OR MODIFICATIONS.

components/riscv/include/riscv/encoding.h:

Copyright (c) 2010-2017, The Regents of the University of California
(Regents).  All Rights Reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.
3. Neither the name of the Regents nor the
   names of its contributors may be used to endorse or promote products
   derived from this software without specific prior written permission.

   IN NO EVENT SHALL REGENTS BE LIABLE TO ANY PARTY FOR DIRECT, INDIRECT,
   SPECIAL, INCIDENTAL, OR CONSEQUENTIAL DAMAGES, INCLUDING LOST PROFITS, ARISING
   OUT OF THE USE OF THIS SOFTWARE AND ITS DOCUMENTATION, EVEN IF REGENTS HAS
   BEEN ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

   REGENTS SPECIFICALLY DISCLAIMS ANY WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
   THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
   PURPOSE. THE SOFTWARE AND ACCOMPANYING DOCUMENTATION, IF ANY, PROVIDED
   HEREUNDER IS PROVIDED "AS IS". REGENTS HAS NO OBLIGATION TO PROVIDE
   MAINTENANCE, SUPPORT, UPDATES, ENHANCEMENTS, OR MODIFICATIONS.
````

Create `third_party/esp-idf-bootloader/PROVENANCE.md`:

````markdown
# Provenance

| File | Size | SHA-256 |
|---|---|---|
| `esp32c3/bootloader.bin` | 21,040 bytes | `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3` |

`esp32c3/bootloader.bin` is the 2nd-stage bootloader that `tools/reference-firmware/build.sh`
builds, byte for byte (the build checks it against the same SHA-256, `SHA256SUMS.inputs`, and the
CI job `reference-boot` compares this file with the bootloader of the merged image it builds).

| Item | Value |
|---|---|
| Build image | `espressif/idf:v5.5.2` (Docker Hub) |
| ESP-IDF | tag `v5.5.2`, commit `30aaf64524299d3bde422ca9a2848090d1bc5d0f` |
| Toolchain | `riscv32-esp-elf-gcc` `esp-14.2.0_20251107` (GCC 14.2.0), from the image |
| Project | `$IDF_PATH/examples/get-started/hello_world`, unmodified; the bootloader does not depend on the application |
| Command | `idf.py set-target esp32c3 && idf.py build`, in the container, network off |
| Output | `build/bootloader/bootloader.bin` |

The configuration, `tools/reference-firmware/sdkconfig.defaults`, is the only input that is not
ESP-IDF's own:

```
CONFIG_IDF_TARGET="esp32c3"
CONFIG_ESPTOOLPY_FLASHSIZE_16MB=y
CONFIG_ESPTOOLPY_FLASHMODE_DIO=y
CONFIG_ESPTOOLPY_FLASHFREQ_80M=y
CONFIG_ESP_DEFAULT_CPU_FREQ_MHZ_160=y
CONFIG_APP_REPRODUCIBLE_BUILD=y
```

Every other option keeps the default of ESP-IDF v5.5.2: log level INFO, no application rollback,
minimum chip revision v0.3, no secure boot, no flash encryption. The build is reproducible: clean
builds, a build from another path and a build with the compiler cache off give the same file
(`docs/research/sp1b-test-oracle.md`, section 2; `docs/research/shipped-bootloader-and-firmware-composition.md`,
section D).

**Unmodified ESP-IDF sources, configuration only.** No source file was changed.

**What inkemu changes when it writes the file into a flash.** For a device whose flash is not
16 MiB, inkemu writes the image with the flash-size nibble of its header (byte 3, high nibble) set
to the size of that flash and its appended SHA-256 computed again, as `esptool.py` does to any image
written at offset 0 (`esptool/cmds.py` of esptool v4.11). That is a modification of the object
form (Apache License 2.0, section 4 b). This file is never changed: the rewrite happens in memory,
in `firmware::bootloader::ShippedBootloader::for_flash`.
````

````diff
--- a/NOTICE
+++ b/NOTICE
@@ -3,3 +3,9 @@
 This product includes register descriptions derived from Espressif's CMSIS-SVD files
 (https://github.com/espressif/svd), Copyright Espressif Systems (Shanghai) Co., Ltd.,
 licensed under the Apache License, Version 2.0. See third_party/espressif-svd/.
+
+This product includes the ESP-IDF 2nd-stage bootloader (https://github.com/espressif/esp-idf,
+v5.5.2), Copyright Espressif Systems (Shanghai) Co., Ltd., licensed under the Apache License,
+Version 2.0, which contains code from two headers distributed under a BSD-3-Clause licence
+(Copyright The Regents of the University of California; the libfemto authors).
+See third_party/esp-idf-bootloader/.
````

- [ ] **Step 2: Write the failing tests**

Create `crates/firmware/src/bootloader.rs` with its tests only:

````rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::FirmwareImage;

    #[test]
    fn every_shipped_bootloader_is_what_its_entry_says() {
        for entry in &SHIPPED {
            assert_eq!(sha256_hex(entry.bytes), entry.sha256, "{}", entry.id);
            let image = FirmwareImage::parse(entry.bytes).unwrap();
            assert_eq!(image.chip, entry.chip, "{}", entry.id);
            assert_eq!(image.spi_mode, entry.flash_mode, "{}", entry.id);
            assert_eq!(
                image.min_chip_rev_full, entry.min_chip_revision,
                "{}",
                entry.id
            );
            assert_eq!(
                image.len,
                entry.bytes.len(),
                "{}: ends with its trailer",
                entry.id
            );
            assert!(image.hash_appended, "{}", entry.id);
            let desc = BootloaderDesc::parse(&image.segments[0].data).unwrap();
            assert_eq!(desc.idf_ver, entry.idf_version, "{}", entry.id);
            assert!(
                desc.date_time.is_empty(),
                "{}: a reproducible build",
                entry.id
            );
            assert!(
                image.app_desc.is_none(),
                "{}: a bootloader, not an application",
                entry.id
            );
        }
    }

    #[test]
    fn the_esp32c3_dio_bootloader_is_the_reference_build() {
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        assert_eq!(entry.bytes.len(), 21_040);
        assert_eq!(
            entry.sha256, "601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3",
            "tools/reference-firmware/SHA256SUMS.inputs"
        );
        assert!(shipped(ChipId::Esp32C3, SpiMode::Qio).is_none());
        assert!(shipped(ChipId::Esp32S3, SpiMode::Dio).is_none());
    }

    #[test]
    fn for_a_16_mib_flash_the_image_is_written_as_it_is() {
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        assert_eq!(entry.for_flash(FlashSize::Mb16), entry.bytes);
    }

    #[test]
    fn for_a_4_mib_flash_the_header_and_digest_are_those_of_the_4_mib_build() {
        // The reference configuration built with CONFIG_ESPTOOLPY_FLASHSIZE_4MB, measured (SB §B).
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        let image = entry.for_flash(FlashSize::Mb4);
        assert_eq!(
            sha256_hex(&image),
            "d6421484b1abfd57d3bf50cf493d5b29b35edfc6f67e0690d13db8e4f56109c5"
        );
        let parsed = FirmwareImage::parse(&image).unwrap();
        assert_eq!(parsed.flash_size, FlashSize::Mb4);
        assert_eq!(parsed.flash_freq_raw, 0xF, "80 MHz is kept");
        let changed = image
            .iter()
            .zip(entry.bytes)
            .filter(|(new, old)| new != old)
            .count();
        assert!(changed <= 33, "byte 3 and the digest only: {changed}");
    }

    #[test]
    fn a_bootloader_descriptor_is_read_where_esp_idf_puts_it() {
        let entry = shipped(ChipId::Esp32C3, SpiMode::Dio).unwrap();
        let first_segment = &entry.bytes[0x20..0x20 + 80];
        let desc = BootloaderDesc::parse(first_segment).unwrap();
        assert_eq!(
            desc,
            BootloaderDesc {
                version: 1,
                idf_ver: "v5.5.2".into(),
                date_time: String::new()
            }
        );
        assert!(BootloaderDesc::parse(&first_segment[..79]).is_none());
        let mut other = first_segment.to_vec();
        other[0] = 0x51;
        assert!(BootloaderDesc::parse(&other).is_none());
    }
}
````


````diff
--- a/crates/firmware/src/chip.rs
+++ b/crates/firmware/src/chip.rs
@@ -173,7 +173,11 @@
             let size = FlashSize::from_nibble(nibble).unwrap();
             assert_eq!(size.megabytes(), mb);
             assert_eq!(size.bytes(), u64::from(mb) * 1024 * 1024);
-        }
+            assert_eq!(size.nibble(), nibble);
+            assert_eq!(FlashSize::from_bytes(size.bytes()), Some(size));
+        }
+        assert_eq!(FlashSize::from_bytes(3 << 20), None);
+        assert_eq!(FlashSize::from_bytes(0), None);
         assert_eq!(FlashSize::from_nibble(8), None);
     }
 
````


````diff
--- a/crates/firmware/src/lib.rs
+++ b/crates/firmware/src/lib.rs
@@ -2,6 +2,7 @@
 #![cfg_attr(test, allow(clippy::unwrap_used))]
 
 pub mod app_desc;
+pub mod bootloader;
 #[cfg(any(test, feature = "test-support"))]
 pub mod builder;
 pub mod chip;
@@ -11,6 +12,7 @@
 pub mod partition;
 
 pub use app_desc::AppDesc;
+pub use bootloader::{BootloaderDesc, SHIPPED, ShippedBootloader};
 pub use chip::{ChipId, FlashSize, SpiMode};
 pub use error::FirmwareError;
 pub use flash_image::{FlashImageInfo, check_flash_image, has_partition_table};
````


- [ ] **Step 3: Run them and see them fail**

```bash
cargo test -p firmware --lib > /tmp/t3-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t3-red.log
```

Expected: `exit=101`; the crate does not compile: ``unresolved imports `bootloader::BootloaderDesc`, `bootloader::SHIPPED`, `bootloader::ShippedBootloader` ``, ``cannot find value `SHIPPED` in this scope``, ``cannot find type `ChipId` in this scope``.

- [ ] **Step 4: The registry and the header written for a flash size**

Put this above the tests of `crates/firmware/src/bootloader.rs`:

````rust
//! The 2nd-stage bootloader: the descriptor it carries, and the ones inkemu ships.
//!
//! inkemu ships one bootloader per chip family and flash-mode family, built from unmodified
//! ESP-IDF sources (`third_party/esp-idf-bootloader/`, ADR 0005). A device profile selects one by
//! what it states, its chip family; the size of its flash is written into the header when the
//! flash is composed (`docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** in
//! the comments of this crate, §B).
use crate::chip::{ChipId, FlashSize, SpiMode};
use sha2::{Digest, Sha256};

/// `ESP_BOOTLOADER_DESC_MAGIC_BYTE` (`esp_bootloader_desc.h`): the first byte of
/// `esp_bootloader_desc_t`, which starts the first segment of a bootloader built by ESP-IDF.
pub const BOOTLOADER_DESC_MAGIC: u8 = 80;
/// Size of `esp_bootloader_desc_t`.
const DESC_LEN: usize = 80;
const DESC_VERSION: usize = 4;
/// String fields are `(offset, field size)`, NUL-terminated within the field.
const DESC_IDF_VER: (usize, usize) = (8, 32);
const DESC_DATE_TIME: (usize, usize) = (40, 24);
/// Header byte 3: flash size in the high nibble, flash frequency in the low one.
const SIZE_FREQ_OFFSET: usize = 3;
/// Header byte 23: non-zero when a SHA-256 digest ends the image.
const HASH_APPENDED_OFFSET: usize = 23;
const SHA256_LEN: usize = 32;

/// What `esp_bootloader_desc_t` says of the build of a bootloader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootloaderDesc {
    pub version: u32,
    /// The ESP-IDF version, as `git describe` gave it: `v5.5.2`, `v5.5.2-729-g87912cd291`.
    pub idf_ver: String,
    /// Empty for a reproducible build.
    pub date_time: String,
}

fn c_string(bytes: &[u8], (at, len): (usize, usize)) -> String {
    let field = &bytes[at..at + len];
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

impl BootloaderDesc {
    /// Returns `None` when the segment does not start with a bootloader descriptor.
    #[must_use]
    pub fn parse(segment: &[u8]) -> Option<BootloaderDesc> {
        if segment.len() < DESC_LEN || segment[0] != BOOTLOADER_DESC_MAGIC {
            return None;
        }
        let version = u32::from_le_bytes([
            segment[DESC_VERSION],
            segment[DESC_VERSION + 1],
            segment[DESC_VERSION + 2],
            segment[DESC_VERSION + 3],
        ]);
        Some(BootloaderDesc {
            version,
            idf_ver: c_string(segment, DESC_IDF_VER),
            date_time: c_string(segment, DESC_DATE_TIME),
        })
    }
}

/// The SHA-256 of `bytes`, in lower-case hexadecimal.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(2 * SHA256_LEN), |mut text, byte| {
            text.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
            text.push(char::from(b"0123456789abcdef"[usize::from(byte & 0xF)]));
            text
        })
}

/// A bootloader inkemu carries in its binary. Its fields are what the image itself says (a test
/// holds them to the bytes); they are here so that a profile, or a front end, can select and show
/// one without reading its header.
#[derive(Debug, PartialEq, Eq)]
pub struct ShippedBootloader {
    /// A stable name: what a summary prints and a front end can store.
    pub id: &'static str,
    pub chip: ChipId,
    /// The flash mode its code drives: one binary per flash-mode family (a QIO build is other
    /// code, whose header still says DIO: SB §B).
    pub flash_mode: SpiMode,
    /// The ESP-IDF release it was built from.
    pub idf_version: &'static str,
    /// The lowest chip revision it starts on, `major * 100 + minor` (header `min_chip_rev_full`).
    /// Below it the bootloader resets the chip for ever (SB §B).
    pub min_chip_revision: u16,
    /// SHA-256 of `bytes`, in lower-case hexadecimal.
    pub sha256: &'static str,
    pub bytes: &'static [u8],
}

/// Every bootloader inkemu ships. Adding a chip family is one entry here and its directory in
/// `third_party/esp-idf-bootloader/`; a device with a chip already here needs its profile only.
pub static SHIPPED: [ShippedBootloader; 1] = [ShippedBootloader {
    id: "esp-idf-v5.5.2-esp32c3-dio",
    chip: ChipId::Esp32C3,
    flash_mode: SpiMode::Dio,
    idf_version: "v5.5.2",
    min_chip_revision: 3,
    sha256: "601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3",
    bytes: include_bytes!("../../../third_party/esp-idf-bootloader/esp32c3/bootloader.bin"),
}];

/// The bootloader inkemu ships for `chip` and the flash-mode family of `flash_mode`, if any.
#[must_use]
pub fn shipped(chip: ChipId, flash_mode: SpiMode) -> Option<&'static ShippedBootloader> {
    SHIPPED
        .iter()
        .find(|entry| entry.chip == chip && entry.flash_mode == flash_mode)
}

impl ShippedBootloader {
    /// The bytes to write at offset 0 of a flash of `size`: the shipped image with the flash size
    /// of its header set to `size` and its appended digest computed again, as `esptool.py` does
    /// to any image written at offset 0 (SB §B, `esptool/cmds.py:263-345`). The bootloader hands
    /// that size to the application, which aborts when the chip it finds is smaller. An image
    /// whose header already says `size` comes back as it is.
    #[must_use]
    pub fn for_flash(&self, size: FlashSize) -> Vec<u8> {
        let mut image = self.bytes.to_vec();
        let header = image[SIZE_FREQ_OFFSET];
        let wanted = (size.nibble() << 4) | (header & 0x0F);
        if header == wanted {
            return image;
        }
        image[SIZE_FREQ_OFFSET] = wanted;
        if image[HASH_APPENDED_OFFSET] != 0 {
            // INVARIANT: a shipped image ends with its digest (a test holds every entry to it).
            let hashed = image.len() - SHA256_LEN;
            let digest = Sha256::digest(&image[..hashed]);
            image[hashed..].copy_from_slice(&digest);
        }
        image
    }
}

````


````diff
--- a/crates/firmware/src/chip.rs
+++ b/crates/firmware/src/chip.rs
@@ -78,6 +78,20 @@
             7 => FlashSize::Mb128,
             _ => return None,
         })
+    }
+
+    /// The code of the header nibble: the inverse of [`FlashSize::from_nibble`].
+    #[must_use]
+    pub const fn nibble(self) -> u8 {
+        self as u8
+    }
+
+    /// The flash size of exactly `bytes` bytes, if it is one the header can state.
+    #[must_use]
+    pub fn from_bytes(bytes: u64) -> Option<FlashSize> {
+        (0..=7)
+            .filter_map(FlashSize::from_nibble)
+            .find(|size| size.bytes() == bytes)
     }
 
     #[must_use]
````


- [ ] **Step 5: Run the tests**

```bash
cargo test -p firmware --lib > /tmp/t3.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t3.log
cargo deny check > /tmp/t3-deny.log 2>&1; echo "exit=$?"
cargo test --workspace > /tmp/t3-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t3-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `64 passed`; `cargo deny check` `exit=0`; then `exit=0` and `1016 60` (tests passed and ignored in the workspace).

- [ ] **Step 6: Mutations**

Each row: apply the change, run `cargo test -p firmware --lib`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `bootloader.rs`, in `for_flash`: delete `image[hashed..].copy_from_slice(&digest);` | `for_a_4_mib_flash_the_header_and_digest_are_those_of_the_4_mib_build` |
| 2 | `if header == wanted {` → `if true {` | `for_a_4_mib_flash_the_header_and_digest_are_those_of_the_4_mib_build` |
| 3 | `.find(\|entry\| entry.chip == chip && entry.flash_mode == flash_mode)` → `.find(\|entry\| entry.chip == chip)` | `the_esp32c3_dio_bootloader_is_the_reference_build` |
| 4 | `min_chip_revision: 3,` → `4` | `every_shipped_bootloader_is_what_its_entry_says` |

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/firmware/src/bootloader.rs third_party/esp-idf-bootloader/LICENSE third_party/esp-idf-bootloader/NOTICE third_party/esp-idf-bootloader/PROVENANCE.md third_party/esp-idf-bootloader/esp32c3/bootloader.bin
/usr/bin/git commit -m "feat(firmware): inkemu ships the ESP-IDF v5.5.2 bootloader of the ESP32-C3, with its licence and provenance" -- NOTICE crates/firmware/src/bootloader.rs crates/firmware/src/chip.rs crates/firmware/src/lib.rs third_party/esp-idf-bootloader/LICENSE third_party/esp-idf-bootloader/NOTICE third_party/esp-idf-bootloader/PROVENANCE.md third_party/esp-idf-bootloader/esp32c3/bootloader.bin
```

Expected: three `exit=0`, then the commit.

---

## Task 4: A bootloader given by the user is checked against the device before it is written

`--bootloader` (Task 9) and `DeviceProfile::compose_flash` with a bootloader (Task 7) take any file. What the mask ROM would refuse is refused here, before boot (RFB §F steps 1 to 6), and so is what would make the bootloader reset the chip for ever (a minimum revision above the device's, SB §B), what would make the application abort (a header that claims more flash than the device), and a file that is something else: an encrypted update, a whole flash image, an application. A wrong appended digest and a smaller flash are warnings (C17, C18). The bootloader is never rewritten. Its identity is what the summary prints: the shipped one, one known by its hash (the Arduino-ESP32 prebuilt of the CrossPoint web flasher, FW §I), or what its descriptor says.

**Files:**
- Modify: `crates/firmware/src/bootloader.rs`
- Modify: `crates/firmware/src/builder.rs`
- Create: `crates/firmware/src/device.rs`
- Modify: `crates/firmware/src/lib.rs`

**Interfaces:**
- Consumes: `FirmwareImage::parse_reporting_digest`, `has_partition_table`, `SHIPPED`, `BootloaderDesc`.
- Produces: `firmware::device::{Device { chip: ChipId, chip_revision: u16, efuse_block_revision: u16, flash_size: FlashSize }, revision(u16) -> String}`; `firmware::bootloader::{KnownBootloader { name, sha256 }, KNOWN, BootloaderIdentity::{Shipped(&'static ShippedBootloader), Known(&'static KnownBootloader), Described { idf_ver, date_time, sha256 }, Unknown { sha256 }} (Display), BootloaderWarning::{DigestMismatch, SmallerFlash { header, device }}, BootloaderReport { identity, warnings }, BOOTLOADER_REMEDY_VERBS, BootloaderError (14 variants), check_bootloader(&[u8], &Device) -> Result<BootloaderReport, BootloaderError>}`; `builder::{bootloader_desc_bytes(idf_ver, date_time) -> Vec<u8>, bootloader_spec() -> ImageSpec}`.

**Bounds on untrusted input.** A file longer than `0x8000` bytes is refused before it is parsed (`TooLong`), after the partition-table look that names a whole image; the parse is bounded as in Task 2 (16 segments at most, each checked against what remains); the segment loop runs at most 16 times; the identity hashes the file once.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/firmware/src/bootloader.rs
+++ b/crates/firmware/src/bootloader.rs
@@ -141,6 +141,9 @@
 #[cfg(test)]
 mod tests {
     use super::*;
+    use crate::builder::{
+        ImageSpec, app_desc_bytes, bootloader_spec, build_flash_image, build_image, refresh_digest,
+    };
     use crate::image::FirmwareImage;
 
     #[test]
@@ -233,4 +236,291 @@
         other[0] = 0x51;
         assert!(BootloaderDesc::parse(&other).is_none());
     }
-}
+
+    const X4: Device = Device {
+        chip: ChipId::Esp32C3,
+        chip_revision: 3,
+        efuse_block_revision: 102,
+        flash_size: FlashSize::Mb16,
+    };
+
+    fn shipped_bytes() -> &'static [u8] {
+        SHIPPED[0].bytes
+    }
+
+    fn refusal(bytes: &[u8]) -> BootloaderError {
+        check_bootloader(bytes, &X4).unwrap_err()
+    }
+
+    #[test]
+    fn the_shipped_bootloader_passes_and_is_named() {
+        let report = check_bootloader(shipped_bytes(), &X4).unwrap();
+        assert_eq!(report.identity, BootloaderIdentity::Shipped(&SHIPPED[0]));
+        assert_eq!(report.warnings, []);
+        assert_eq!(
+            report.identity.to_string(),
+            "the ESP-IDF v5.5.2 bootloader inkemu ships (esp-idf-v5.5.2-esp32c3-dio)"
+        );
+    }
+
+    #[test]
+    fn another_bootloader_is_named_by_its_descriptor_or_its_hash() {
+        let built = build_image(&bootloader_spec());
+        let report = check_bootloader(&built, &X4).unwrap();
+        let sha256 = sha256_hex(&built);
+        assert_eq!(
+            report.identity,
+            BootloaderIdentity::Described {
+                idf_ver: "v5.5.2-test".into(),
+                date_time: "Oct 13 2026 10:00:00".into(),
+                sha256: sha256.clone()
+            }
+        );
+        assert_eq!(
+            report.identity.to_string(),
+            format!(
+                "a bootloader of ESP-IDF v5.5.2-test built Oct 13 2026 10:00:00, sha256 {sha256}"
+            )
+        );
+        let mut spec = bootloader_spec();
+        spec.segments[0].1[0] = 0; // no descriptor
+        let bare = build_image(&spec);
+        assert_eq!(
+            check_bootloader(&bare, &X4).unwrap().identity,
+            BootloaderIdentity::Unknown {
+                sha256: sha256_hex(&bare)
+            }
+        );
+    }
+
+    #[test]
+    fn a_whole_flash_image_an_ota_update_or_a_long_file_is_not_a_bootloader() {
+        assert_eq!(
+            refusal(&build_flash_image(&bootloader_spec())),
+            BootloaderError::FlashImage
+        );
+        let mut xota = b"XOTA".to_vec();
+        xota.resize(512, 0);
+        assert_eq!(refusal(&xota), BootloaderError::EncryptedOta);
+        let mut long = build_image(&bootloader_spec());
+        long.resize(0x8001, 0xFF);
+        assert_eq!(refusal(&long), BootloaderError::TooLong { len: 0x8001 });
+        long.truncate(0x8000);
+        assert!(
+            check_bootloader(&long, &X4).is_ok(),
+            "erased bytes up to the table"
+        );
+        assert_eq!(refusal(&[]), BootloaderError::TooShort { len: 0 });
+        assert_eq!(refusal(&[0xE9; 23]), BootloaderError::TooShort { len: 23 });
+        assert_eq!(
+            refusal(&[0x7F; 64]),
+            BootloaderError::NotAnImage { found: 0x7F }
+        );
+    }
+
+    #[test]
+    fn an_application_given_as_the_bootloader_is_named_as_one() {
+        assert_eq!(
+            refusal(&build_image(&ImageSpec::default())),
+            BootloaderError::Application
+        );
+        // Without its descriptor, its flash-mapped segment still gives it away.
+        let spec = ImageSpec {
+            segments: vec![(0x4200_0020, vec![0; 8])],
+            ..ImageSpec::default()
+        };
+        assert_eq!(
+            refusal(&build_image(&spec)),
+            BootloaderError::FlashMapped {
+                index: 0,
+                addr: 0x4200_0020
+            }
+        );
+    }
+
+    #[test]
+    fn a_segment_the_rom_would_not_load_is_refused() {
+        for (addr, len) in [
+            (0x3FCD_E15C, 8),    // ends inside the ROM's stack
+            (0x3FCD_F660, 8),    // starts inside it
+            (0x5000_0000, 8),    // RTC memory
+            (0x3FCD_FFFC, 0x8),  // runs past the end of DRAM
+            (0x403D_FFF8, 0x10), // runs past the end of IRAM
+            (0x3FC8_0002, 8),    // not word-aligned
+        ] {
+            let mut spec = bootloader_spec();
+            spec.segments.push((addr, vec![0; len]));
+            let index = spec.segments.len() - 1;
+            assert_eq!(
+                refusal(&build_image(&spec)),
+                BootloaderError::Unloadable { index, addr, len },
+                "{addr:#x}+{len:#x}"
+            );
+        }
+    }
+
+    #[test]
+    fn a_header_the_rom_or_the_device_would_refuse_is_refused() {
+        let with = |at: usize, value: u8| {
+            let mut bytes = build_image(&bootloader_spec());
+            bytes[at] = value;
+            refresh_digest(&mut bytes);
+            bytes
+        };
+        assert_eq!(refusal(&with(2, 4)), BootloaderError::BadMode { raw: 4 });
+        assert_eq!(
+            refusal(&with(3, 0x43)),
+            BootloaderError::BadSpeed { raw: 3 }
+        );
+        assert!(matches!(
+            refusal(&with(1, 17)),
+            BootloaderError::Damaged(FirmwareError::TooManySegments { .. })
+        ));
+        assert_eq!(
+            refusal(&with(12, 9)),
+            BootloaderError::ChipMismatch {
+                expected: ChipId::Esp32C3,
+                found: ChipId::Esp32S3
+            }
+        );
+        assert_eq!(
+            refusal(&with(3, 0x5F)),
+            BootloaderError::LargerFlash {
+                header: 32,
+                device: 16
+            }
+        );
+    }
+
+    #[test]
+    fn a_bootloader_that_requires_a_later_chip_is_refused_before_it_resets_for_ever() {
+        let mut bytes = build_image(&bootloader_spec());
+        bytes[15..17].copy_from_slice(&4u16.to_le_bytes());
+        refresh_digest(&mut bytes);
+        let err = refusal(&bytes);
+        assert_eq!(
+            err,
+            BootloaderError::RevisionTooHigh {
+                required: 4,
+                device: 3
+            }
+        );
+        let text = err.to_string();
+        assert!(
+            text.contains("requires chip revision v0.4 or later and the device is v0.3")
+                && text.contains("reset the chip for ever"),
+            "{text}"
+        );
+        bytes[15..17].copy_from_slice(&3u16.to_le_bytes());
+        refresh_digest(&mut bytes);
+        assert!(check_bootloader(&bytes, &X4).is_ok(), "v0.3 on v0.3");
+    }
+
+    #[test]
+    fn a_smaller_flash_and_a_wrong_digest_are_warnings() {
+        let mut bytes = build_image(&bootloader_spec());
+        bytes[3] = 0x2F;
+        refresh_digest(&mut bytes);
+        assert_eq!(
+            check_bootloader(&bytes, &X4).unwrap().warnings,
+            [BootloaderWarning::SmallerFlash {
+                header: 4,
+                device: 16
+            }]
+        );
+        let last = bytes.len() - 1;
+        bytes[last] ^= 1;
+        assert_eq!(
+            check_bootloader(&bytes, &X4).unwrap().warnings,
+            [
+                BootloaderWarning::SmallerFlash {
+                    header: 4,
+                    device: 16
+                },
+                BootloaderWarning::DigestMismatch
+            ]
+        );
+        // A checksum is never a warning: the ROM refuses the image.
+        let spec = ImageSpec {
+            hash_appended: false,
+            ..bootloader_spec()
+        };
+        let mut bytes = build_image(&spec);
+        let last = bytes.len() - 1;
+        bytes[last] ^= 1;
+        assert!(matches!(
+            refusal(&bytes),
+            BootloaderError::Damaged(FirmwareError::ChecksumMismatch { .. })
+        ));
+    }
+
+    #[test]
+    fn a_known_bootloader_is_named() {
+        assert_eq!(
+            KNOWN[0].sha256,
+            "6e6b0d386095f0783e9f0513ea29ea2ce413790e7f7153a06a2b04e2b5f59fb6"
+        );
+        let identity = BootloaderIdentity::Known(&KNOWN[0]);
+        assert!(identity.to_string().contains("Arduino-ESP32"), "{identity}");
+        // A descriptor with an application's magic is an application, whatever else it holds.
+        let mut spec = bootloader_spec();
+        spec.segments[0].1 = app_desc_bytes("1", "demo", "v5.5.2");
+        assert_eq!(refusal(&build_image(&spec)), BootloaderError::Application);
+    }
+
+    #[test]
+    fn every_refusal_and_warning_states_a_remedy() {
+        let errors = [
+            BootloaderError::EncryptedOta,
+            BootloaderError::FlashImage,
+            BootloaderError::TooLong { len: 1 },
+            BootloaderError::TooShort { len: 1 },
+            BootloaderError::NotAnImage { found: 1 },
+            BootloaderError::Application,
+            BootloaderError::BadMode { raw: 4 },
+            BootloaderError::BadSpeed { raw: 3 },
+            BootloaderError::Damaged(FirmwareError::HashMismatch),
+            BootloaderError::FlashMapped { index: 0, addr: 0 },
+            BootloaderError::Unloadable {
+                index: 0,
+                addr: 0,
+                len: 0,
+            },
+            BootloaderError::ChipMismatch {
+                expected: ChipId::Esp32C3,
+                found: ChipId::Esp32S3,
+            },
+            BootloaderError::RevisionTooHigh {
+                required: 4,
+                device: 3,
+            },
+            BootloaderError::LargerFlash {
+                header: 32,
+                device: 16,
+            },
+        ];
+        let verbs: Vec<&str> = BOOTLOADER_REMEDY_VERBS
+            .iter()
+            .chain(crate::error::REMEDY_VERBS.iter())
+            .copied()
+            .collect();
+        for message in errors.iter().map(ToString::to_string).chain(
+            [
+                BootloaderWarning::DigestMismatch,
+                BootloaderWarning::SmallerFlash {
+                    header: 4,
+                    device: 16,
+                },
+            ]
+            .iter()
+            .map(ToString::to_string),
+        ) {
+            assert!(
+                verbs
+                    .iter()
+                    .any(|verb| message.contains(&format!("; {verb} "))),
+                "no remedy in: {message}"
+            );
+        }
+    }
+}
````


Create `crates/firmware/src/device.rs` with its tests only:

````rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revision_prints_as_esp_idf_prints_it() {
        assert_eq!(revision(3), "v0.3");
        assert_eq!(revision(101), "v1.1");
        assert_eq!(revision(199), "v1.99");
    }
}
````


````diff
--- a/crates/firmware/src/lib.rs
+++ b/crates/firmware/src/lib.rs
@@ -6,14 +6,19 @@
 #[cfg(any(test, feature = "test-support"))]
 pub mod builder;
 pub mod chip;
+pub mod device;
 pub mod error;
 pub mod flash_image;
 pub mod image;
 pub mod partition;
 
 pub use app_desc::AppDesc;
-pub use bootloader::{BootloaderDesc, SHIPPED, ShippedBootloader};
+pub use bootloader::{
+    BootloaderDesc, BootloaderError, BootloaderIdentity, BootloaderReport, BootloaderWarning,
+    SHIPPED, ShippedBootloader, check_bootloader,
+};
 pub use chip::{ChipId, FlashSize, SpiMode};
+pub use device::Device;
 pub use error::FirmwareError;
 pub use flash_image::{FlashImageInfo, check_flash_image, has_partition_table};
 pub use image::{FirmwareImage, Segment};
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p firmware --lib > /tmp/t4-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t4-red.log
```

Expected: `exit=101`; the crate does not compile: ``unresolved import `device::Device` ``, ``unresolved import `crate::builder::bootloader_spec` ``, ``unresolved imports `bootloader::BootloaderError`, …``.

- [ ] **Step 3: The checks, the identity and the test bootloader**

````diff
--- a/crates/firmware/src/bootloader.rs
+++ b/crates/firmware/src/bootloader.rs
@@ -6,7 +6,12 @@
 //! flash is composed (`docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** in
 //! the comments of this crate, §B).
 use crate::chip::{ChipId, FlashSize, SpiMode};
+use crate::device::{Device, revision};
+use crate::error::FirmwareError;
+use crate::image::{FirmwareImage, HEADER_LEN, IMAGE_MAGIC, XOTA_MAGIC};
+use crate::partition::PARTITION_TABLE_OFFSET;
 use sha2::{Digest, Sha256};
+use std::fmt;
 
 /// `ESP_BOOTLOADER_DESC_MAGIC_BYTE` (`esp_bootloader_desc.h`): the first byte of
 /// `esp_bootloader_desc_t`, which starts the first segment of a bootloader built by ESP-IDF.
@@ -22,6 +27,21 @@
 /// Header byte 23: non-zero when a SHA-256 digest ends the image.
 const HASH_APPENDED_OFFSET: usize = 23;
 const SHA256_LEN: usize = 32;
+/// Header byte 2: the SPI mode. A bootloader runs in QIO, QOUT, DIO or DOUT (0 to 3).
+const SPI_MODE_OFFSET: usize = 2;
+const LAST_BOOTLOADER_MODE: u8 = 3;
+/// Speed codes the mask ROM accepts in the low nibble of byte 3: 0 to 2, and 15
+/// (`docs/research/esp32c3-rom-flash-boot.md`, **RFB** below, §F step 1).
+const ROM_SPEEDS: [u8; 4] = [0, 1, 2, 0xF];
+/// The longest segment the ROM loads (RFB §F step 4).
+const MAX_SEGMENT_LEN: usize = 0x10_0000;
+/// The ROM's stack and data, where it refuses to load a segment (RFB §F step 5).
+const ROM_RESERVED: (u32, u32) = (0x3FCD_E160, 0x3FCD_F664);
+/// Where a bootloader's segments are loaded: internal SRAM, through its data bus (DRAM) or its
+/// instruction bus (IRAM) (ESP32-C3 Technical Reference Manual, "System and Memory").
+const RAM: [(u32, u32); 2] = [(0x3FC8_0000, 0x3FCE_0000), (0x4037_C000, 0x403E_0000)];
+/// The two windows the cache maps the flash to: an application's code and read-only data.
+const FLASH_WINDOWS: [(u32, u32); 2] = [(0x3C00_0000, 0x3C80_0000), (0x4200_0000, 0x4280_0000)];
 
 /// What `esp_bootloader_desc_t` says of the build of a bootloader.
 #[derive(Clone, Debug, PartialEq, Eq)]
@@ -136,6 +156,277 @@
         }
         image
     }
+}
+
+/// A bootloader inkemu recognises by its SHA-256, and neither ships nor fetches
+/// (`docs/research/flash-writes-and-crosspoint-boot.md`, §N).
+#[derive(Debug, PartialEq, Eq)]
+pub struct KnownBootloader {
+    pub name: &'static str,
+    /// SHA-256 of the file, in lower-case hexadecimal.
+    pub sha256: &'static str,
+}
+
+/// Bootloaders a user is likely to give, named in the summary of a run.
+pub static KNOWN: [KnownBootloader; 1] = [KnownBootloader {
+    name: "the Arduino-ESP32 prebuilt bootloader of the CrossPoint web flasher (ESP-IDF v5.5.2-729-g87912cd291)",
+    sha256: "6e6b0d386095f0783e9f0513ea29ea2ce413790e7f7153a06a2b04e2b5f59fb6",
+}];
+
+/// What a bootloader is known as.
+#[derive(Clone, Debug, PartialEq, Eq)]
+pub enum BootloaderIdentity {
+    /// One inkemu ships.
+    Shipped(&'static ShippedBootloader),
+    /// One of [`KNOWN`].
+    Known(&'static KnownBootloader),
+    /// Another, as its descriptor describes it.
+    Described {
+        idf_ver: String,
+        date_time: String,
+        sha256: String,
+    },
+    /// Another, without a descriptor (ESP-IDF before v5.1, or not ESP-IDF).
+    Unknown { sha256: String },
+}
+
+impl fmt::Display for BootloaderIdentity {
+    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
+        match self {
+            BootloaderIdentity::Shipped(entry) => write!(
+                f,
+                "the ESP-IDF {} bootloader inkemu ships ({})",
+                entry.idf_version, entry.id
+            ),
+            BootloaderIdentity::Known(known) => f.write_str(known.name),
+            BootloaderIdentity::Described {
+                idf_ver,
+                date_time,
+                sha256,
+            } if date_time.is_empty() => {
+                write!(f, "a bootloader of ESP-IDF {idf_ver}, sha256 {sha256}")
+            }
+            BootloaderIdentity::Described {
+                idf_ver,
+                date_time,
+                sha256,
+            } => write!(
+                f,
+                "a bootloader of ESP-IDF {idf_ver} built {date_time}, sha256 {sha256}"
+            ),
+            BootloaderIdentity::Unknown { sha256 } => {
+                write!(f, "a bootloader without a descriptor, sha256 {sha256}")
+            }
+        }
+    }
+}
+
+/// What may not go as the user expects, and does not stop the run.
+#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
+pub enum BootloaderWarning {
+    #[error(
+        "the SHA-256 appended to the bootloader does not match it; the mask ROM boots it anyway, as a chip without secure boot does, and prints four lines that say so; rebuild the bootloader if it was not changed on purpose"
+    )]
+    DigestMismatch,
+    #[error(
+        "the header of the bootloader says {header} MB of flash and the device has {device} MB; the application is told {header} MB and uses no more; rebuild the bootloader for {device} MB to use the whole flash"
+    )]
+    SmallerFlash { header: u32, device: u32 },
+}
+
+/// What the check of a bootloader found.
+#[derive(Clone, Debug, PartialEq, Eq)]
+pub struct BootloaderReport {
+    pub identity: BootloaderIdentity,
+    pub warnings: Vec<BootloaderWarning>,
+}
+
+/// Imperatives that open the remedy clause of a [`BootloaderError`] (`"; <verb> ..."`).
+pub const BOOTLOADER_REMEDY_VERBS: [&str; 4] = ["use", "give", "rebuild", "choose"];
+
+/// Why a file cannot be the bootloader of a device. The messages name the option of `inkemu run`
+/// that takes what the file is.
+#[derive(Debug, thiserror::Error, PartialEq, Eq)]
+pub enum BootloaderError {
+    #[error(
+        "the bootloader is an encrypted OTA update (.xota), not a bootloader; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
+    )]
+    EncryptedOta,
+    #[error(
+        "the bootloader is a whole flash image (a partition table follows it at 0x8000), not a bootloader alone; use the file as the flash (`inkemu run --flash`)"
+    )]
+    FlashImage,
+    #[error(
+        "the bootloader is {len} bytes, more than the 32768 bytes below the partition table; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
+    )]
+    TooLong { len: usize },
+    #[error(
+        "the bootloader is empty or shorter than an image header ({len} bytes); give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
+    )]
+    TooShort { len: usize },
+    #[error(
+        "the bootloader is not an ESP-IDF image: its first byte is {found:#04x}, expected 0xe9; give the bootloader.bin of a build, or no bootloader to use the one inkemu ships"
+    )]
+    NotAnImage { found: u8 },
+    #[error(
+        "the bootloader is an application image (it carries an application descriptor at 0x20); use it as the firmware (`--firmware`), and give no bootloader or the bootloader.bin of its build"
+    )]
+    Application,
+    #[error(
+        "the bootloader declares SPI mode {raw}, which no bootloader runs in (0 to 3: QIO, QOUT, DIO, DOUT); rebuild it, or use the one inkemu ships"
+    )]
+    BadMode { raw: u8 },
+    #[error(
+        "the bootloader declares flash speed code {raw}, which the mask ROM refuses (0, 1, 2 or 15); rebuild it, or use the one inkemu ships"
+    )]
+    BadSpeed { raw: u8 },
+    #[error("the bootloader is damaged: {0}")]
+    Damaged(FirmwareError),
+    #[error(
+        "segment {index} of the bootloader is loaded at {addr:#010x}, in a window of the flash, as an application's code is; use the file as the firmware (`--firmware`), with the bootloader of its build"
+    )]
+    FlashMapped { index: usize, addr: u32 },
+    #[error(
+        "segment {index} of the bootloader ({len} bytes at {addr:#010x}) is not where the mask ROM loads a bootloader (word-aligned, at most 1 MiB, in internal RAM, clear of 0x3fcde160-0x3fcdf663); rebuild it, or use the one inkemu ships"
+    )]
+    Unloadable { index: usize, addr: u32, len: usize },
+    #[error(
+        "the bootloader is built for {found}, and the device uses {expected}; give a bootloader built for {expected}, or no bootloader to use the one inkemu ships"
+    )]
+    ChipMismatch { expected: ChipId, found: ChipId },
+    #[error(
+        "the bootloader requires chip revision {} or later and the device is {}: it would reset the chip for ever; give a bootloader built for {} (CONFIG_ESP32C3_REV_MIN), or choose a device of revision {} or later",
+        revision(*required), revision(*device), revision(*device), revision(*required)
+    )]
+    RevisionTooHigh { required: u16, device: u16 },
+    #[error(
+        "the header of the bootloader says {header} MB of flash and the device has {device} MB: the application would abort when it finds the smaller chip; give a bootloader built for {device} MB, or no bootloader to use the one inkemu ships"
+    )]
+    LargerFlash { header: u32, device: u32 },
+}
+
+fn within(range: (u32, u32), addr: u32, len: usize) -> bool {
+    let end = u64::from(addr) + len as u64;
+    u64::from(range.0) <= u64::from(addr) && end <= u64::from(range.1)
+}
+
+fn overlaps(range: (u32, u32), addr: u32, len: usize) -> bool {
+    let end = u64::from(addr) + len as u64;
+    u64::from(addr) < u64::from(range.1) && end > u64::from(range.0)
+}
+
+/// What `bytes` is known as: shipped, known, or described by its descriptor.
+fn identify(bytes: &[u8], image: &FirmwareImage) -> BootloaderIdentity {
+    let sha256 = sha256_hex(bytes);
+    if let Some(entry) = SHIPPED.iter().find(|entry| entry.sha256 == sha256) {
+        return BootloaderIdentity::Shipped(entry);
+    }
+    if let Some(known) = KNOWN.iter().find(|known| known.sha256 == sha256) {
+        return BootloaderIdentity::Known(known);
+    }
+    match image
+        .segments
+        .first()
+        .and_then(|s| BootloaderDesc::parse(&s.data))
+    {
+        Some(desc) => BootloaderIdentity::Described {
+            idf_ver: desc.idf_ver,
+            date_time: desc.date_time,
+            sha256,
+        },
+        None => BootloaderIdentity::Unknown { sha256 },
+    }
+}
+
+/// Checks a bootloader given by the user before it is written at offset 0 of the flash of
+/// `device`: what the mask ROM would refuse, what would make the bootloader reset the chip or
+/// the application abort, and a file that is something else (SB §G). It is never rewritten.
+///
+/// # Errors
+/// [`BootloaderError`]: the cause, then what to give instead.
+pub fn check_bootloader(
+    bytes: &[u8],
+    device: &Device,
+) -> Result<BootloaderReport, BootloaderError> {
+    if bytes.starts_with(XOTA_MAGIC) {
+        return Err(BootloaderError::EncryptedOta);
+    }
+    if crate::flash_image::has_partition_table(bytes) {
+        return Err(BootloaderError::FlashImage);
+    }
+    if bytes.len() > PARTITION_TABLE_OFFSET as usize {
+        return Err(BootloaderError::TooLong { len: bytes.len() });
+    }
+    if bytes.len() < HEADER_LEN {
+        return Err(BootloaderError::TooShort { len: bytes.len() });
+    }
+    // INVARIANT: every index below is under HEADER_LEN, which the length check just covered.
+    if bytes[0] != IMAGE_MAGIC {
+        return Err(BootloaderError::NotAnImage { found: bytes[0] });
+    }
+    if bytes[SPI_MODE_OFFSET] > LAST_BOOTLOADER_MODE {
+        return Err(BootloaderError::BadMode {
+            raw: bytes[SPI_MODE_OFFSET],
+        });
+    }
+    let speed = bytes[SIZE_FREQ_OFFSET] & 0x0F;
+    if !ROM_SPEEDS.contains(&speed) {
+        return Err(BootloaderError::BadSpeed { raw: speed });
+    }
+    let (image, digest_matches) =
+        FirmwareImage::parse_reporting_digest(bytes).map_err(BootloaderError::Damaged)?;
+    if image.app_desc.is_some() {
+        return Err(BootloaderError::Application);
+    }
+    for (index, segment) in image.segments.iter().enumerate() {
+        let (addr, len) = (segment.load_addr, segment.data.len());
+        if FLASH_WINDOWS
+            .iter()
+            .any(|&window| overlaps(window, addr, len))
+        {
+            return Err(BootloaderError::FlashMapped { index, addr });
+        }
+        let loadable = addr % 4 == 0
+            && len <= MAX_SEGMENT_LEN
+            && RAM.iter().any(|&ram| within(ram, addr, len))
+            && !overlaps(ROM_RESERVED, addr, len);
+        if !loadable {
+            return Err(BootloaderError::Unloadable { index, addr, len });
+        }
+    }
+    if image.chip != device.chip {
+        return Err(BootloaderError::ChipMismatch {
+            expected: device.chip,
+            found: image.chip,
+        });
+    }
+    if image.min_chip_rev_full > device.chip_revision {
+        return Err(BootloaderError::RevisionTooHigh {
+            required: image.min_chip_rev_full,
+            device: device.chip_revision,
+        });
+    }
+    let (header, flash) = (image.flash_size.megabytes(), device.flash_size.megabytes());
+    if header > flash {
+        return Err(BootloaderError::LargerFlash {
+            header,
+            device: flash,
+        });
+    }
+    let mut warnings = Vec::new();
+    if header < flash {
+        warnings.push(BootloaderWarning::SmallerFlash {
+            header,
+            device: flash,
+        });
+    }
+    if !digest_matches {
+        warnings.push(BootloaderWarning::DigestMismatch);
+    }
+    Ok(BootloaderReport {
+        identity: identify(bytes, &image),
+        warnings,
+    })
 }
 
 #[cfg(test)]
````


````diff
--- a/crates/firmware/src/builder.rs
+++ b/crates/firmware/src/builder.rs
@@ -111,6 +111,39 @@
     flash
 }
 
+/// An 80-byte `esp_bootloader_desc_t` with the given strings (truncated to their field sizes).
+#[must_use]
+pub fn bootloader_desc_bytes(idf_ver: &str, date_time: &str) -> Vec<u8> {
+    let mut d = vec![0u8; 80];
+    d[0] = crate::bootloader::BOOTLOADER_DESC_MAGIC;
+    d[4..8].copy_from_slice(&1u32.to_le_bytes());
+    let put = |d: &mut Vec<u8>, at: usize, len: usize, s: &str| {
+        let b = s.as_bytes();
+        let n = b.len().min(len - 1);
+        d[at..at + n].copy_from_slice(&b[..n]);
+    };
+    put(&mut d, 8, 32, idf_ver);
+    put(&mut d, 40, 24, date_time);
+    d
+}
+
+/// A small bootloader: its descriptor in DRAM, then four bytes of code in IRAM, at the addresses
+/// of ESP-IDF's (`docs/research/flash-writes-and-crosspoint-boot.md`, §I).
+#[must_use]
+pub fn bootloader_spec() -> ImageSpec {
+    ImageSpec {
+        entry: 0x403C_BF10,
+        segments: vec![
+            (
+                0x3FCD_5820,
+                bootloader_desc_bytes("v5.5.2-test", "Oct 13 2026 10:00:00"),
+            ),
+            (0x403C_BF10, vec![0x6F, 0, 0, 0]),
+        ],
+        ..ImageSpec::default()
+    }
+}
+
 /// A 256-byte `esp_app_desc_t` with the given strings (truncated to their field sizes).
 #[must_use]
 pub fn app_desc_bytes(version: &str, project: &str, idf: &str) -> Vec<u8> {
````


Put this above the tests of `crates/firmware/src/device.rs`:

````rust
//! The device a flash is composed for, as the checks of a bootloader and of an application see it.
use crate::chip::{ChipId, FlashSize};

/// `efuse_hal_chip_revision()` and the image headers count a revision `major * 100 + minor`.
const MAJOR_WEIGHT: u16 = 100;

/// What a bootloader and an application are checked against before a flash is composed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub chip: ChipId,
    /// The chip revision, `major * 100 + minor`.
    pub chip_revision: u16,
    /// The eFuse block revision the chip reports, `major * 100 + minor`.
    pub efuse_block_revision: u16,
    pub flash_size: FlashSize,
}

/// `v<major>.<minor>` of a revision counted `major * 100 + minor`, as ESP-IDF prints it.
#[must_use]
pub fn revision(full: u16) -> String {
    format!("v{}.{}", full / MAJOR_WEIGHT, full % MAJOR_WEIGHT)
}

````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p firmware --lib > /tmp/t4.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t4.log
cargo test --workspace > /tmp/t4-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t4-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `75 passed`; then `exit=0` and `1027 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test -p firmware --lib`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `bootloader.rs`: delete the `if image.app_desc.is_some() { return Err(BootloaderError::Application); }` statement | `an_application_given_as_the_bootloader_is_named_as_one`, `a_known_bootloader_is_named` |
| 2 | the condition `if FLASH_WINDOWS.iter().any(\|&window\| overlaps(window, addr, len))` → `if false` | `an_application_given_as_the_bootloader_is_named_as_one` |
| 3 | `&& !overlaps(ROM_RESERVED, addr, len);` → `;` | `a_segment_the_rom_would_not_load_is_refused` |
| 4 | `if image.min_chip_rev_full > device.chip_revision {` → `>=` | `a_bootloader_that_requires_a_later_chip_is_refused_before_it_resets_for_ever`, `the_shipped_bootloader_passes_and_is_named` |
| 5 | `if header > flash {` → `>=` | `the_shipped_bootloader_passes_and_is_named`, `another_bootloader_is_named_by_its_descriptor_or_its_hash` and two more |
| 6 | `if bytes.len() > PARTITION_TABLE_OFFSET as usize {` → `>=` | `a_whole_flash_image_an_ota_update_or_a_long_file_is_not_a_bootloader` |
| 7 | delete `warnings.push(BootloaderWarning::DigestMismatch);` | `a_smaller_flash_and_a_wrong_digest_are_warnings` |
| 8 | `SHIPPED.iter().find(\|entry\| entry.sha256 == sha256)` → `SHIPPED.iter().find(\|_\| false)` | `the_shipped_bootloader_passes_and_is_named` |
| 9 | `if !ROM_SPEEDS.contains(&speed) {` → `if false {` | `a_header_the_rom_or_the_device_would_refuse_is_refused` |
| 10 | `if bytes[SPI_MODE_OFFSET] > LAST_BOOTLOADER_MODE {` → `if false {` | `a_header_the_rom_or_the_device_would_refuse_is_refused` |
| 11 | `if image.chip != device.chip {` → `if false {` | `a_header_the_rom_or_the_device_would_refuse_is_refused` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/firmware/src/device.rs
/usr/bin/git commit -m "feat(firmware): a bootloader given by the user is checked against the device before it is written" -- crates/firmware/src/bootloader.rs crates/firmware/src/builder.rs crates/firmware/src/device.rs crates/firmware/src/lib.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 5: An application is checked against the device and its slot before it is written

What the bootloader would refuse in an application is refused before boot (FW §M, SB §C, spec §10.3 criterion 7): the image structure, checksum and digest; the chip id; the chip-revision bounds of its header (a maximum of 0 or 65535 means none, `IS_FIELD_SET` in `IDF:bootloader_support/src/bootloader_common_loader.c`); the eFuse block-revision bounds of its descriptor, against the revision the chip reports; a header that claims more flash than the device (spec §5.1). It must fit its slot, the file length being what is written. A file that is something else is named for what it is: an encrypted update, a whole flash image, a bootloader. An application without a descriptor is refused (C23). It is never rewritten.

**Files:**
- Create: `crates/firmware/src/application.rs`
- Modify: `crates/firmware/src/lib.rs`

**Interfaces:**
- Consumes: `FirmwareImage::parse`, `AppDesc` (Task 2), `BootloaderDesc`, `Device`, `revision` (Task 4), `Partition`.
- Produces: `firmware::application::{ApplicationReport { desc: AppDesc, image_len: usize }, APPLICATION_REMEDY_VERBS, ApplicationError (14 variants), check_application(&[u8], &Device, &Partition) -> Result<ApplicationReport, ApplicationError>}`, re-exported from the crate root.

**Bounds on untrusted input.** The parse is that of Task 2; the size check compares the length of a slice the caller read with its own bound (Task 9: the flash size).

- [ ] **Step 1: Write the failing tests**

Create `crates/firmware/src/application.rs` with its tests only:

````rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{
        ImageSpec, app_desc_bytes, bootloader_spec, build_flash_image, build_image, refresh_digest,
    };
    use crate::chip::FlashSize;
    use crate::partition::{SUBTYPE_OTA_0, TYPE_APP};

    const X4: Device = Device {
        chip: ChipId::Esp32C3,
        chip_revision: 3,
        efuse_block_revision: 102,
        flash_size: FlashSize::Mb16,
    };

    fn slot(size: u32) -> Partition {
        Partition {
            name: "app0".into(),
            ptype: TYPE_APP,
            subtype: SUBTYPE_OTA_0,
            offset: 0x1_0000,
            size,
            flags: 0,
        }
    }

    fn refusal(bytes: &[u8]) -> ApplicationError {
        check_application(bytes, &X4, &slot(0x64_0000)).unwrap_err()
    }

    /// The default test application with its header bytes `at..` set to `value` (LE), digest
    /// refreshed.
    fn with_header(at: usize, value: u16) -> Vec<u8> {
        let mut bytes = build_image(&ImageSpec::default());
        bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
        refresh_digest(&mut bytes);
        bytes
    }

    /// The default test application whose descriptor accepts eFuse blocks `min..=max`.
    fn with_efuse_blocks(min: u16, max: u16) -> Vec<u8> {
        let mut desc = app_desc_bytes("1.0.0", "demo", "v5.5.2");
        desc[176..178].copy_from_slice(&min.to_le_bytes());
        desc[178..180].copy_from_slice(&max.to_le_bytes());
        let mut spec = ImageSpec::default();
        spec.segments[0].1 = desc;
        build_image(&spec)
    }

    #[test]
    fn an_application_for_the_device_passes_with_its_descriptor() {
        let bytes = build_image(&ImageSpec::default());
        let report = check_application(&bytes, &X4, &slot(0x64_0000)).unwrap();
        assert_eq!(report.desc.project_name, "demo");
        assert_eq!(report.image_len, bytes.len());
    }

    #[test]
    fn what_is_not_an_application_is_named_for_what_it_is() {
        let mut xota = b"XOTA".to_vec();
        xota.resize(512, 0);
        assert_eq!(refusal(&xota), ApplicationError::EncryptedOta);
        assert_eq!(
            refusal(&build_flash_image(&ImageSpec::default())),
            ApplicationError::FlashImage
        );
        assert_eq!(
            refusal(&build_image(&bootloader_spec())),
            ApplicationError::Bootloader
        );
        let spec = ImageSpec {
            segments: vec![(0x4038_0000, vec![0; 8])],
            ..ImageSpec::default()
        };
        assert_eq!(refusal(&build_image(&spec)), ApplicationError::NoDescriptor);
        assert_eq!(refusal(&[0xE9; 10]), ApplicationError::TooShort { len: 10 });
        assert_eq!(
            refusal(&[0x00; 64]),
            ApplicationError::NotAnImage { found: 0 }
        );
    }

    #[test]
    fn a_damaged_image_is_refused_with_what_is_wrong() {
        let mut bytes = build_image(&ImageSpec::default());
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            refusal(&bytes),
            ApplicationError::Damaged(FirmwareError::HashMismatch)
        );
        let mut lying = build_image(&ImageSpec::default());
        lying[28..32].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(matches!(
            refusal(&lying),
            ApplicationError::Damaged(FirmwareError::TruncatedSegment { .. })
        ));
    }

    #[test]
    fn an_application_for_another_chip_is_refused_with_both_chips() {
        let err = refusal(&with_header(12, 9));
        assert_eq!(
            err,
            ApplicationError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3
            }
        );
        let text = err.to_string();
        assert!(
            text.contains("ESP32-S3") && text.contains("ESP32-C3"),
            "{text}"
        );
    }

    #[test]
    fn an_application_larger_than_its_slot_is_refused() {
        let bytes = build_image(&ImageSpec::default());
        let len = bytes.len();
        let fits = u32::try_from(len).unwrap();
        assert!(
            check_application(&bytes, &X4, &slot(fits)).is_ok(),
            "exactly the slot"
        );
        assert_eq!(
            check_application(&bytes, &X4, &slot(fits - 1)).unwrap_err(),
            ApplicationError::TooLarge {
                len,
                slot: "app0".into(),
                size: fits - 1
            }
        );
    }

    #[test]
    fn an_application_declaring_more_flash_than_the_device_is_refused() {
        let mut bytes = build_image(&ImageSpec::default());
        bytes[3] = 0x5F;
        refresh_digest(&mut bytes);
        assert_eq!(
            refusal(&bytes),
            ApplicationError::LargerFlash {
                header: 32,
                device: 16
            }
        );
    }

    #[test]
    fn the_chip_revision_bounds_are_those_the_bootloader_checks() {
        // Header bytes 15..17: min_chip_rev_full; 17..19: max_chip_rev_full.
        assert_eq!(
            refusal(&with_header(15, 4)),
            ApplicationError::ChipTooOld {
                required: 4,
                device: 3
            }
        );
        assert!(check_application(&with_header(15, 3), &X4, &slot(0x64_0000)).is_ok());
        assert_eq!(
            refusal(&with_header(17, 2)),
            ApplicationError::ChipTooNew {
                accepted: 2,
                device: 3
            }
        );
        for max in [0, u16::MAX, 3] {
            assert!(
                check_application(&with_header(17, max), &X4, &slot(0x64_0000)).is_ok(),
                "max {max}"
            );
        }
    }

    #[test]
    fn the_efuse_block_bounds_are_those_the_bootloader_checks() {
        assert_eq!(
            refusal(&with_efuse_blocks(103, 199)),
            ApplicationError::EfuseBlockTooOld {
                required: 103,
                device: 102
            }
        );
        assert_eq!(
            refusal(&with_efuse_blocks(0, 101)),
            ApplicationError::EfuseBlockTooNew {
                accepted: 101,
                device: 102
            }
        );
        for (min, max) in [(0, 0), (u16::MAX, u16::MAX), (102, 102), (0, 199)] {
            assert!(
                check_application(&with_efuse_blocks(min, max), &X4, &slot(0x64_0000)).is_ok(),
                "{min}..={max}"
            );
        }
    }

    #[test]
    fn every_refusal_states_a_remedy() {
        let errors = [
            ApplicationError::EncryptedOta,
            ApplicationError::FlashImage,
            ApplicationError::TooShort { len: 1 },
            ApplicationError::NotAnImage { found: 1 },
            ApplicationError::Damaged(FirmwareError::HashMismatch),
            ApplicationError::Bootloader,
            ApplicationError::NoDescriptor,
            ApplicationError::ChipMismatch {
                expected: ChipId::Esp32C3,
                found: ChipId::Esp32S3,
            },
            ApplicationError::TooLarge {
                len: 2,
                slot: "app0".into(),
                size: 1,
            },
            ApplicationError::LargerFlash {
                header: 32,
                device: 16,
            },
            ApplicationError::ChipTooOld {
                required: 4,
                device: 3,
            },
            ApplicationError::ChipTooNew {
                accepted: 2,
                device: 3,
            },
            ApplicationError::EfuseBlockTooOld {
                required: 103,
                device: 102,
            },
            ApplicationError::EfuseBlockTooNew {
                accepted: 101,
                device: 102,
            },
        ];
        let verbs: Vec<&str> = APPLICATION_REMEDY_VERBS
            .iter()
            .chain(crate::error::REMEDY_VERBS.iter())
            .copied()
            .collect();
        for err in errors {
            let message = err.to_string();
            assert!(
                verbs
                    .iter()
                    .any(|verb| message.contains(&format!("; {verb} "))),
                "no remedy in: {message}"
            );
        }
    }
}
````


````diff
--- a/crates/firmware/src/lib.rs
+++ b/crates/firmware/src/lib.rs
@@ -2,6 +2,7 @@
 #![cfg_attr(test, allow(clippy::unwrap_used))]
 
 pub mod app_desc;
+pub mod application;
 pub mod bootloader;
 #[cfg(any(test, feature = "test-support"))]
 pub mod builder;
@@ -13,6 +14,7 @@
 pub mod partition;
 
 pub use app_desc::AppDesc;
+pub use application::{ApplicationError, ApplicationReport, check_application};
 pub use bootloader::{
     BootloaderDesc, BootloaderError, BootloaderIdentity, BootloaderReport, BootloaderWarning,
     SHIPPED, ShippedBootloader, check_bootloader,
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p firmware --lib > /tmp/t5-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t5-red.log
```

Expected: `exit=101`; ``unresolved imports `application::ApplicationError`, `application::ApplicationReport`, `application::check_application` ``, ``cannot find type `Device` in this scope``.

- [ ] **Step 3: The checks of an application**

Put this above the tests of `crates/firmware/src/application.rs`:

````rust
//! Checks of an application image before it is written into a flash: what the 2nd-stage
//! bootloader would refuse, what does not fit, and a file that is something else (spec §5.1;
//! `docs/research/flash-writes-and-crosspoint-boot.md`, **FW** below, §M;
//! `docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** below, §C).
use crate::app_desc::AppDesc;
use crate::bootloader::BootloaderDesc;
use crate::chip::ChipId;
use crate::device::{Device, revision};
use crate::error::FirmwareError;
use crate::image::{FirmwareImage, HEADER_LEN, IMAGE_MAGIC, XOTA_MAGIC};
use crate::partition::Partition;

/// The bootloader ignores a revision bound of 0 or 65535 (`IS_FIELD_SET`,
/// `bootloader_common_loader.c` of ESP-IDF v5.5.2).
const fn is_set(bound: u16) -> bool {
    bound != 0 && bound != u16::MAX
}

/// What the check of an application found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationReport {
    pub desc: AppDesc,
    /// Bytes of the image; the file may hold more (a partition dump holds the rest of its slot).
    pub image_len: usize,
}

/// Imperatives that open the remedy clause of an [`ApplicationError`] (`"; <verb> ..."`).
pub const APPLICATION_REMEDY_VERBS: [&str; 3] = ["use", "give", "choose"];

/// Why a file cannot be the application of a device. The messages name the option of
/// `inkemu run` that takes what the file is.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplicationError {
    #[error(
        "the firmware is an encrypted OTA update (.xota), which inkemu cannot read; use the plain application .bin of a release, not the encrypted OTA file"
    )]
    EncryptedOta,
    #[error(
        "the firmware is a whole flash image (a dump, or the output of `esptool.py merge_bin`: a partition table at 0x8000), not an application; use it as the flash (`--flash`)"
    )]
    FlashImage,
    #[error(
        "the firmware is empty or shorter than an image header ({len} bytes); use the application .bin of an ESP-IDF build or release"
    )]
    TooShort { len: usize },
    #[error(
        "the firmware is not an ESP-IDF image: its first byte is {found:#04x}, expected 0xe9 (an encrypted or compressed update, or another format); use the plain application .bin of an ESP-IDF build or release"
    )]
    NotAnImage { found: u8 },
    #[error("the firmware is damaged: {0}")]
    Damaged(FirmwareError),
    #[error(
        "the firmware is a 2nd-stage bootloader (it carries a bootloader descriptor at 0x20), not an application; give it as the bootloader (`--bootloader`), and the application .bin as the firmware"
    )]
    Bootloader,
    #[error(
        "the firmware has no application descriptor at 0x20 (esp_app_desc_t), which every ESP-IDF application carries and the bootloader reads; use the application .bin of an ESP-IDF build"
    )]
    NoDescriptor,
    #[error(
        "the firmware is built for {found} and the device uses {expected}; choose a device with that chip, or use the firmware built for {expected}"
    )]
    ChipMismatch { expected: ChipId, found: ChipId },
    #[error(
        "the firmware is {len} bytes and its slot, {slot}, holds {size}; use an application that fits, or choose a device whose partition table has a larger slot"
    )]
    TooLarge { len: usize, slot: String, size: u32 },
    #[error(
        "the header of the firmware says {header} MB of flash and the device has {device} MB; use the firmware built for {device} MB, or choose a device with at least {header} MB"
    )]
    LargerFlash { header: u32, device: u32 },
    #[error(
        "the firmware requires chip revision {} or later and the device is {}: the bootloader would refuse to start it; use a build for {} (CONFIG_ESP32C3_REV_MIN), or choose a device of revision {} or later",
        revision(*required), revision(*device), revision(*device), revision(*required)
    )]
    ChipTooOld { required: u16, device: u16 },
    #[error(
        "the firmware accepts chip revisions up to {} and the device is {}: the bootloader would refuse to start it; use a build that accepts {}, or choose a device of revision {} or earlier",
        revision(*accepted), revision(*device), revision(*device), revision(*accepted)
    )]
    ChipTooNew { accepted: u16, device: u16 },
    #[error(
        "the firmware requires eFuse block revision {} or later and the chip has {}: the bootloader would refuse to start it; use a build that accepts {}",
        revision(*required), revision(*device), revision(*device)
    )]
    EfuseBlockTooOld { required: u16, device: u16 },
    #[error(
        "the firmware accepts eFuse block revisions up to {} and the chip has {}: the bootloader would refuse to start it; use a build that accepts {}",
        revision(*accepted), revision(*device), revision(*device)
    )]
    EfuseBlockTooNew { accepted: u16, device: u16 },
}

/// Checks the application `bytes` for `device`, to be written into `slot`. The application is
/// never rewritten: what the bootloader would refuse is refused here, before boot.
///
/// # Errors
/// [`ApplicationError`]: the cause, then what to give instead.
pub fn check_application(
    bytes: &[u8],
    device: &Device,
    slot: &Partition,
) -> Result<ApplicationReport, ApplicationError> {
    if bytes.starts_with(XOTA_MAGIC) {
        return Err(ApplicationError::EncryptedOta);
    }
    if crate::flash_image::has_partition_table(bytes) {
        return Err(ApplicationError::FlashImage);
    }
    if bytes.len() < HEADER_LEN {
        return Err(ApplicationError::TooShort { len: bytes.len() });
    }
    if bytes[0] != IMAGE_MAGIC {
        return Err(ApplicationError::NotAnImage { found: bytes[0] });
    }
    let image = FirmwareImage::parse(bytes).map_err(ApplicationError::Damaged)?;
    let first = image.segments.first().map(|s| s.data.as_slice());
    let Some(desc) = image.app_desc.clone() else {
        return Err(if first.and_then(BootloaderDesc::parse).is_some() {
            ApplicationError::Bootloader
        } else {
            ApplicationError::NoDescriptor
        });
    };
    if image.chip != device.chip {
        return Err(ApplicationError::ChipMismatch {
            expected: device.chip,
            found: image.chip,
        });
    }
    if bytes.len() > slot.size as usize {
        return Err(ApplicationError::TooLarge {
            len: bytes.len(),
            slot: slot.name.clone(),
            size: slot.size,
        });
    }
    let (header, flash) = (image.flash_size.megabytes(), device.flash_size.megabytes());
    if header > flash {
        return Err(ApplicationError::LargerFlash {
            header,
            device: flash,
        });
    }
    check_revisions(&image, &desc, device)?;
    Ok(ApplicationReport {
        desc,
        image_len: image.len,
    })
}

/// The revision bounds the bootloader checks before it starts an application: the chip's
/// (`bootloader_common_check_chip_revision_validity`) and the eFuse block's
/// (`bootloader_common_check_efuse_blk_validity`, SB §C).
fn check_revisions(
    image: &FirmwareImage,
    desc: &AppDesc,
    device: &Device,
) -> Result<(), ApplicationError> {
    let chip = device.chip_revision;
    if image.min_chip_rev_full > chip {
        return Err(ApplicationError::ChipTooOld {
            required: image.min_chip_rev_full,
            device: chip,
        });
    }
    if is_set(image.max_chip_rev_full) && chip > image.max_chip_rev_full {
        return Err(ApplicationError::ChipTooNew {
            accepted: image.max_chip_rev_full,
            device: chip,
        });
    }
    let block = device.efuse_block_revision;
    if is_set(desc.min_efuse_block_revision) && desc.min_efuse_block_revision > block {
        return Err(ApplicationError::EfuseBlockTooOld {
            required: desc.min_efuse_block_revision,
            device: block,
        });
    }
    if is_set(desc.max_efuse_block_revision) && block > desc.max_efuse_block_revision {
        return Err(ApplicationError::EfuseBlockTooNew {
            accepted: desc.max_efuse_block_revision,
            device: block,
        });
    }
    Ok(())
}

````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p firmware --lib > /tmp/t5.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t5.log
cargo test --workspace > /tmp/t5-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t5-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `84 passed`; then `exit=0` and `1036 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test -p firmware --lib application`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `application.rs`: `bound != 0 && bound != u16::MAX` → `bound != u16::MAX` | `an_application_for_the_device_passes_with_its_descriptor` and three more |
| 2 | `bound != 0 && bound != u16::MAX` → `bound != 0` | `the_efuse_block_bounds_are_those_the_bootloader_checks` |
| 3 | `if bytes.len() > slot.size as usize {` → `if bytes.len() > slot.size as usize + 1 {` | `an_application_larger_than_its_slot_is_refused` |
| 4 | `if image.min_chip_rev_full > chip {` → `>=` | `the_chip_revision_bounds_are_those_the_bootloader_checks` |
| 5 | the `min_efuse_block_revision` condition → `if false {` | `the_efuse_block_bounds_are_those_the_bootloader_checks` |
| 6 | the `max_efuse_block_revision` condition → `if false {` | `the_efuse_block_bounds_are_those_the_bootloader_checks` |
| 7 | `if image.chip != device.chip {` → `if false {` | `an_application_for_another_chip_is_refused_with_both_chips` |
| 8 | `if first.and_then(BootloaderDesc::parse).is_some() {` → `if false {` | `what_is_not_an_application_is_named_for_what_it_is` |
| 9 | `if header > flash {` → `if false {` | `an_application_declaring_more_flash_than_the_device_is_refused` |
| 10 | delete the `has_partition_table` check of `check_application` | `what_is_not_an_application_is_named_for_what_it_is` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/firmware/src/application.rs
/usr/bin/git commit -m "feat(firmware): an application is checked against the device and its slot before it is written" -- crates/firmware/src/application.rs crates/firmware/src/lib.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 6: Compose a flash from a partition table, an application and a bootloader

Bytes in, bytes out (C8): the bootloader at offset 0 (esptool's `BOOTLOADER_FLASH_OFFSET`, 0 for the C3 and the S3, SB §E), the table at `0x8000`, the application in the slot the bootloader starts on a blank `otadata` (C16), every other byte `0xFF`, the image exactly the flash size. The shipped bootloader is written for the flash size and refused on a chip below its minimum (C5); a given one is checked by Task 4 and written as it is. The same inputs give the same bytes.

**Files:**
- Create: `crates/firmware/src/compose.rs`
- Modify: `crates/firmware/src/lib.rs`
- Modify: `crates/firmware/src/partition.rs`

**Interfaces:**
- Consumes: `check_application` (Task 5), `check_bootloader`, `ShippedBootloader::for_flash` (Tasks 3, 4), `PartitionTable::to_bytes`.
- Produces: `PartitionTable::application_slot(&self) -> Option<&Partition>`; `firmware::compose::{BOOTLOADER_OFFSET, Bootloader::{Shipped(&'static ShippedBootloader), Given(&[u8])}, ComposedFlash { bytes, bootloader: BootloaderIdentity, application: AppDesc, slot: Partition, warnings: Vec<BootloaderWarning> }, ComposeError::{Table, NoApplicationSlot, Application, Bootloader, ShippedNeedsLaterChip { id, required, device }}, compose(&Device, &PartitionTable, &[u8], Bootloader<'_>) -> Result<ComposedFlash, ComposeError>}`, re-exported.

**Bounds on untrusted input.** The one allocation is the flash size of the device, at most 128 MiB by construction of `FlashSize` (16 MiB for every profile); each copy is bounded by what the checks proved: the bootloader at most `0x8000` bytes, the table `0xC00` bytes at `0x8000`, the application no longer than its slot, which the table holds inside the flash.

- [ ] **Step 1: Write the failing tests**

Create `crates/firmware/src/compose.rs` with its tests only:

````rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootloader::{SHIPPED, sha256_hex};
    use crate::builder::{ImageSpec, bootloader_spec, build_image};
    use crate::chip::{ChipId, FlashSize};
    use crate::partition::{SUBTYPE_FACTORY, TYPE_APP, xteink_x4_stock};

    const X4: Device = Device {
        chip: ChipId::Esp32C3,
        chip_revision: 3,
        efuse_block_revision: 102,
        flash_size: FlashSize::Mb16,
    };

    fn application() -> Vec<u8> {
        build_image(&ImageSpec::default())
    }

    fn shipped() -> Bootloader<'static> {
        Bootloader::Shipped(&SHIPPED[0])
    }

    #[test]
    fn the_flash_holds_the_bootloader_the_table_and_the_application_and_nothing_else() {
        let app = application();
        let flash = compose(&X4, &xteink_x4_stock(), &app, shipped()).unwrap();
        let bytes = &flash.bytes;
        assert_eq!(bytes.len(), 16 << 20);
        let boot = SHIPPED[0].bytes;
        assert_eq!(&bytes[..boot.len()], boot);
        let table = xteink_x4_stock().to_bytes(FlashSize::Mb16).unwrap();
        assert_eq!(&bytes[0x8000..0x8000 + table.len()], &table[..]);
        assert_eq!(&bytes[0x1_0000..0x1_0000 + app.len()], &app[..]);
        let written = |at: usize| {
            at < boot.len()
                || (0x8000..0x8000 + table.len()).contains(&at)
                || (0x1_0000..0x1_0000 + app.len()).contains(&at)
        };
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(at, &byte)| written(at) || byte == 0xFF),
            "every other byte is erased, otadata included"
        );
        assert_eq!(flash.slot.name, "app0");
        assert_eq!(flash.application.project_name, "demo");
        assert_eq!(flash.bootloader, BootloaderIdentity::Shipped(&SHIPPED[0]));
    }

    #[test]
    fn the_same_inputs_compose_the_same_flash() {
        let app = application();
        let one = compose(&X4, &xteink_x4_stock(), &app, shipped()).unwrap();
        let two = compose(&X4, &xteink_x4_stock(), &app, shipped()).unwrap();
        assert_eq!(sha256_hex(&one.bytes), sha256_hex(&two.bytes));
    }

    #[test]
    fn a_smaller_flash_gets_the_shipped_bootloader_written_for_its_size() {
        let device = Device {
            flash_size: FlashSize::Mb4,
            ..X4
        };
        // nvs, otadata and a 1 MiB app0: what fits in 4 MiB of the stock layout.
        let mut table = xteink_x4_stock();
        table.entries.truncate(3);
        table.entries[2].size = 0x10_0000;
        let app = build_image(&ImageSpec {
            flash_size_nibble: FlashSize::Mb4.nibble(),
            ..ImageSpec::default()
        });
        let flash = compose(&device, &table, &app, shipped()).unwrap();
        assert_eq!(flash.bytes.len(), 4 << 20);
        assert_eq!(
            flash.bytes[..SHIPPED[0].bytes.len()],
            SHIPPED[0].for_flash(FlashSize::Mb4)[..]
        );
        assert_eq!(flash.bytes[3] >> 4, FlashSize::Mb4.nibble());
    }

    #[test]
    fn a_table_that_does_not_fit_the_flash_or_starts_nothing_is_refused() {
        let device = Device {
            flash_size: FlashSize::Mb4,
            ..X4
        };
        assert!(matches!(
            compose(&device, &xteink_x4_stock(), &application(), shipped()),
            Err(ComposeError::Table(PartitionError::BeyondFlash { .. }))
        ));
        let mut no_app = xteink_x4_stock();
        no_app.entries.retain(|p| p.ptype != TYPE_APP);
        assert_eq!(
            compose(&X4, &no_app, &application(), shipped()),
            Err(ComposeError::NoApplicationSlot)
        );
    }

    #[test]
    fn a_factory_slot_is_where_the_application_goes_when_there_is_one() {
        let mut table = xteink_x4_stock();
        table.entries[3].subtype = SUBTYPE_FACTORY; // app1 becomes the factory application
        let app = application();
        let flash = compose(&X4, &table, &app, shipped()).unwrap();
        assert_eq!(flash.slot.name, "app1");
        assert_eq!(&flash.bytes[0x65_0000..0x65_0000 + app.len()], &app[..]);
        assert!(flash.bytes[0x1_0000..0x2_0000].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn the_shipped_bootloader_is_refused_on_a_chip_it_would_reset_for_ever() {
        let device = Device {
            chip_revision: 2,
            ..X4
        };
        let mut app = application();
        app[15..17].copy_from_slice(&0u16.to_le_bytes());
        let err = compose(&device, &xteink_x4_stock(), &app, shipped()).unwrap_err();
        assert_eq!(
            err,
            ComposeError::ShippedNeedsLaterChip {
                id: "esp-idf-v5.5.2-esp32c3-dio",
                required: 3,
                device: 2
            }
        );
        let text = err.to_string();
        assert!(
            text.contains("v0.3 or later and the device is v0.2")
                && text.contains("; give a bootloader built for v0.2 (`--bootloader`)"),
            "{text}"
        );
        // The same device boots with a bootloader that accepts it.
        let given = build_image(&bootloader_spec());
        assert!(compose(&device, &xteink_x4_stock(), &app, Bootloader::Given(&given)).is_ok());
    }

    #[test]
    fn a_given_bootloader_is_written_as_it_is_with_its_warnings() {
        let mut given = build_image(&bootloader_spec());
        let last = given.len() - 1;
        given[last] ^= 1;
        let flash = compose(
            &X4,
            &xteink_x4_stock(),
            &application(),
            Bootloader::Given(&given),
        )
        .unwrap();
        assert_eq!(&flash.bytes[..given.len()], &given[..]);
        assert_eq!(flash.warnings, [BootloaderWarning::DigestMismatch]);
        assert!(matches!(
            flash.bootloader,
            BootloaderIdentity::Described { .. }
        ));
    }

    #[test]
    fn the_application_and_the_bootloader_are_judged_with_their_own_errors() {
        assert_eq!(
            compose(&X4, &xteink_x4_stock(), &[0u8; 64], shipped()),
            Err(ComposeError::Application(ApplicationError::NotAnImage {
                found: 0
            }))
        );
        let app = application();
        assert_eq!(
            compose(&X4, &xteink_x4_stock(), &app, Bootloader::Given(&app)),
            Err(ComposeError::Bootloader(BootloaderError::Application))
        );
        let s3 = Device {
            chip: ChipId::Esp32S3,
            ..X4
        };
        let mut s3_app = application();
        s3_app[12] = 9;
        crate::builder::refresh_digest(&mut s3_app);
        assert_eq!(
            compose(&s3, &xteink_x4_stock(), &s3_app, shipped()),
            Err(ComposeError::Bootloader(BootloaderError::ChipMismatch {
                expected: ChipId::Esp32S3,
                found: ChipId::Esp32C3
            }))
        );
    }
}
````


````diff
--- a/crates/firmware/src/lib.rs
+++ b/crates/firmware/src/lib.rs
@@ -7,6 +7,7 @@
 #[cfg(any(test, feature = "test-support"))]
 pub mod builder;
 pub mod chip;
+pub mod compose;
 pub mod device;
 pub mod error;
 pub mod flash_image;
@@ -20,6 +21,7 @@
     SHIPPED, ShippedBootloader, check_bootloader,
 };
 pub use chip::{ChipId, FlashSize, SpiMode};
+pub use compose::{Bootloader, ComposeError, ComposedFlash, compose};
 pub use device::Device;
 pub use error::FirmwareError;
 pub use flash_image::{FlashImageInfo, check_flash_image, has_partition_table};
````


````diff
--- a/crates/firmware/src/partition.rs
+++ b/crates/firmware/src/partition.rs
@@ -287,6 +287,25 @@
             (t.entries[2].ptype, t.entries[2].subtype),
             (TYPE_APP, SUBTYPE_OTA_0)
         );
+    }
+
+    #[test]
+    fn the_application_slot_is_the_factory_one_else_ota_0() {
+        assert_eq!(
+            xteink_x4_stock()
+                .application_slot()
+                .map(|p| p.name.as_str()),
+            Some("app0")
+        );
+        let mut with_factory = xteink_x4_stock();
+        with_factory.entries[3].subtype = SUBTYPE_FACTORY;
+        assert_eq!(
+            with_factory.application_slot().map(|p| p.name.as_str()),
+            Some("app1")
+        );
+        let mut ota_1_only = xteink_x4_stock();
+        ota_1_only.entries.remove(2);
+        assert_eq!(ota_1_only.application_slot(), None);
     }
 
     #[test]
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p firmware --lib > /tmp/t6-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t6-red.log
```

Expected: `exit=101`; ``unresolved imports `compose::Bootloader`, `compose::ComposeError`, `compose::ComposedFlash`, `compose::compose` ``.

- [ ] **Step 3: The slot and the composition**

Put this above the tests of `crates/firmware/src/compose.rs`:

````rust
//! A flash composed from an application image: the bootloader at offset 0, the partition table
//! at 0x8000, the application in the slot the bootloader starts, every other byte erased (spec
//! §5.1; `docs/research/shipped-bootloader-and-firmware-composition.md`, **SB** below, §B, §F).
//! Bytes in, bytes out: no file, no host. The same inputs give the same flash, byte for byte.
use crate::app_desc::AppDesc;
use crate::application::{ApplicationError, check_application};
use crate::bootloader::{
    BootloaderError, BootloaderIdentity, BootloaderWarning, ShippedBootloader, check_bootloader,
};
use crate::device::{Device, revision};
use crate::partition::{PARTITION_TABLE_OFFSET, Partition, PartitionError, PartitionTable};

/// Where the 2nd-stage bootloader goes: 0 on the ESP32-C3 and the ESP32-S3 (esptool's
/// `BOOTLOADER_FLASH_OFFSET`; SB §E).
pub const BOOTLOADER_OFFSET: usize = 0;
/// An erased flash byte.
const ERASED: u8 = 0xFF;

/// The bootloader of a composed flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bootloader<'a> {
    /// One inkemu ships: its header is set to the flash size of the device.
    Shipped(&'static ShippedBootloader),
    /// One the user gives: checked, never rewritten.
    Given(&'a [u8]),
}

/// A flash ready to boot, and what it was made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedFlash {
    /// Exactly the flash size of the device.
    pub bytes: Vec<u8>,
    pub bootloader: BootloaderIdentity,
    pub application: AppDesc,
    /// The partition the application was written to.
    pub slot: Partition,
    pub warnings: Vec<BootloaderWarning>,
}

/// Why no flash can be composed. Each message states the cause, then the remedy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ComposeError {
    #[error("the partition table of the device does not fit its flash: {0}")]
    Table(PartitionError),
    #[error(
        "the partition table of the device has neither a factory application nor an ota_0 slot, so the bootloader would find no application to start; choose a device whose table has one"
    )]
    NoApplicationSlot,
    #[error(transparent)]
    Application(ApplicationError),
    #[error(transparent)]
    Bootloader(BootloaderError),
    #[error(
        "the bootloader inkemu ships for this device ({id}) requires chip revision {} or later and the device is {}: it would reset the chip for ever; give a bootloader built for {} (`--bootloader`), or choose a device of revision {} or later",
        revision(*required), revision(*device), revision(*device), revision(*required)
    )]
    ShippedNeedsLaterChip {
        id: &'static str,
        required: u16,
        device: u16,
    },
}

/// Composes the flash of `device` from its partition table, an application and a bootloader.
///
/// The application goes to the slot the bootloader starts on a blank `otadata`
/// ([`PartitionTable::application_slot`]); `otadata` and everything else stay erased, so the
/// first boot is the one of a freshly flashed device.
///
/// # Errors
/// [`ComposeError`]: the table, the application or the bootloader is refused.
pub fn compose(
    device: &Device,
    table: &PartitionTable,
    application: &[u8],
    bootloader: Bootloader<'_>,
) -> Result<ComposedFlash, ComposeError> {
    let table_bytes = table
        .to_bytes(device.flash_size)
        .map_err(ComposeError::Table)?;
    let slot = table
        .application_slot()
        .ok_or(ComposeError::NoApplicationSlot)?
        .clone();
    let report =
        check_application(application, device, &slot).map_err(ComposeError::Application)?;
    let (boot, identity, warnings) = match bootloader {
        Bootloader::Shipped(entry) => {
            if entry.chip != device.chip {
                return Err(ComposeError::Bootloader(BootloaderError::ChipMismatch {
                    expected: device.chip,
                    found: entry.chip,
                }));
            }
            if entry.min_chip_revision > device.chip_revision {
                return Err(ComposeError::ShippedNeedsLaterChip {
                    id: entry.id,
                    required: entry.min_chip_revision,
                    device: device.chip_revision,
                });
            }
            let bytes = entry.for_flash(device.flash_size);
            (bytes, BootloaderIdentity::Shipped(entry), Vec::new())
        }
        Bootloader::Given(bytes) => {
            let checked = check_bootloader(bytes, device).map_err(ComposeError::Bootloader)?;
            (bytes.to_vec(), checked.identity, checked.warnings)
        }
    };
    let Ok(size) = usize::try_from(device.flash_size.bytes()) else {
        // INVARIANT: a flash size is at most 128 MiB, which a usize of 32 bits holds.
        unreachable!("a flash size fits a usize")
    };
    let mut flash = vec![ERASED; size];
    // The bootloader is at most 0x8000 bytes (checked, or shipped), the table 0xC00 bytes at
    // 0x8000, and the application fits its slot, which the table holds inside the flash.
    flash[BOOTLOADER_OFFSET..BOOTLOADER_OFFSET + boot.len()].copy_from_slice(&boot);
    let table_at = PARTITION_TABLE_OFFSET as usize;
    flash[table_at..table_at + table_bytes.len()].copy_from_slice(&table_bytes);
    let app_at = slot.offset as usize;
    flash[app_at..app_at + application.len()].copy_from_slice(application);
    Ok(ComposedFlash {
        bytes: flash,
        bootloader: identity,
        application: report.desc,
        slot,
        warnings,
    })
}

````


````diff
--- a/crates/firmware/src/partition.rs
+++ b/crates/firmware/src/partition.rs
@@ -191,6 +191,20 @@
         out.extend(digest);
         out.resize(PARTITION_TABLE_LEN, ERASED);
         Ok(out)
+    }
+
+    /// The application the 2nd-stage bootloader starts when `otadata` is blank or absent: the
+    /// factory application, else the OTA slot `ota_0` (ESP-IDF v5.5.2,
+    /// `bootloader_utility.c`, `bootloader_utility_get_selected_boot_partition`: "Defaulting to
+    /// factory image", else "No factory image, trying OTA 0").
+    #[must_use]
+    pub fn application_slot(&self) -> Option<&Partition> {
+        let app = |subtype| {
+            self.entries
+                .iter()
+                .find(|p| p.ptype == TYPE_APP && p.subtype == subtype)
+        };
+        app(SUBTYPE_FACTORY).or_else(|| app(SUBTYPE_OTA_0))
     }
 
     /// Reads a table back, verifying its MD5 entry when one is present.
````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p firmware --lib > /tmp/t6.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t6.log
cargo test --workspace > /tmp/t6-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t6-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `93 passed`; then `exit=0` and `1045 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test -p firmware --lib`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `partition.rs`: `app(SUBTYPE_FACTORY).or_else(\|\| app(SUBTYPE_OTA_0))` → `app(SUBTYPE_OTA_0).or_else(\|\| app(SUBTYPE_FACTORY))` | `the_application_slot_is_the_factory_one_else_ota_0`, `a_factory_slot_is_where_the_application_goes_when_there_is_one` |
| 2 | `compose.rs`: `let bytes = entry.for_flash(device.flash_size);` → `let bytes = entry.bytes.to_vec();` | `a_smaller_flash_gets_the_shipped_bootloader_written_for_its_size` |
| 3 | `if entry.min_chip_revision > device.chip_revision {` → `if false {` | `the_shipped_bootloader_is_refused_on_a_chip_it_would_reset_for_ever` |
| 4 | delete `flash[table_at..table_at + table_bytes.len()].copy_from_slice(&table_bytes);` | `the_flash_holds_the_bootloader_the_table_and_the_application_and_nothing_else` |
| 5 | `const ERASED: u8 = 0xFF;` → `0x00` | `the_flash_holds_the_bootloader_the_table_and_the_application_and_nothing_else`, `a_factory_slot_is_where_the_application_goes_when_there_is_one` |
| 6 | `if entry.chip != device.chip {` → `if false {` | `the_application_and_the_bootloader_are_judged_with_their_own_errors` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/firmware/src/compose.rs
/usr/bin/git commit -m "feat(firmware): compose a flash from a partition table, an application and a bootloader" -- crates/firmware/src/compose.rs crates/firmware/src/lib.rs crates/firmware/src/partition.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 7: A profile states its partition table and selects the bootloader inkemu ships for its chip

The two profiles state their table as `[[partition]]` rows (C20): `x4` the stock X4 table, byte for byte the fixture; `x3` the layout of real X3 units with the X4 names (`ASSUMPTION:`). The table is compiled at load and refused there, with its origin, when it does not fit the flash. The profile selects the shipped bootloader of its chip family (DIO for all, C19) and refuses a family without one. `firmware_device()` is the device the checks see, with the eFuse block revision the eFuse model reports (C21, a new `Efuse::block_revision`); `compose_flash` is what a front end calls. The chip-id mapping of `main.rs` moves to `SocFamily::chip_id`, and the CI builds `firmware` and `boards` for WebAssembly.

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `Cargo.lock`
- Modify: `crates/boards/Cargo.toml`
- Modify: `crates/boards/profiles/x3.toml`
- Modify: `crates/boards/profiles/x4.toml`
- Modify: `crates/boards/src/lib.rs`
- Modify: `crates/emu-cli/src/main.rs`
- Modify: `crates/firmware/src/partition.rs`
- Modify: `crates/soc-esp32c3/src/periph/efuse.rs`

**Interfaces:**
- Consumes: `firmware::{compose, Bootloader, ComposeError, ComposedFlash, Device, FlashSize, SpiMode, ChipId}`, `firmware::bootloader::{shipped, ShippedBootloader}`, `firmware::partition::{Partition, PartitionTable, PartitionError, TYPE_APP, TYPE_DATA}`, `soc_esp32c3::periph::Efuse`.
- Produces: `SocFamily::chip_id(self) -> ChipId`; `DeviceProfile { …, pub partitions: PartitionTable, pub bootloader: &'static ShippedBootloader }`; `DeviceProfile::firmware_device(&self) -> Device`; `DeviceProfile::compose_flash(&self, application: &[u8], bootloader: Option<&[u8]>) -> Result<ComposedFlash, ComposeError>`; `ProfileError::{Partitions { origin, source: PartitionError }, NoBootloader { origin, chip: ChipId }}`; `Efuse::block_revision(&self) -> u16`; `PartitionError: Clone`.

**Bounds on untrusted input.** A profile text is untrusted (a front end will load the user's): its rows are deserialized into a vector the size of the text the caller holds, then refused above 94 entries by the table writer (`TooManyEntries`); `subtype_value` compares a name with 2 + 16 + 11 constant names; `offset` and `size` are `u32`.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -345,7 +345,7 @@
     use super::*;
     use soc_esp32c3::test_support::{HANG, flat_rom};
 
-    const VALID: &str = "id = \"demo\"\nname = \"Demo board\"\n\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n\n[flash]\nsize_mib = 4\njedec_id = 0xEF4016\n";
+    const VALID: &str = "id = \"demo\"\nname = \"Demo board\"\n\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n\n[flash]\nsize_mib = 4\njedec_id = 0xEF4016\n\n[[partition]]\nname = \"app\"\ntype = \"app\"\nsubtype = \"factory\"\noffset = 0x10000\nsize = 0x10000\n";
 
     #[test]
     fn builtin_profiles_load_and_carry_their_own_id() {
@@ -371,6 +371,206 @@
     }
 
     #[test]
+    fn every_builtin_profile_states_its_partition_table_and_selects_a_bootloader() {
+        let x4 = builtin("x4").unwrap();
+        assert_eq!(
+            x4.partitions.to_bytes(FlashSize::Mb16).unwrap(),
+            include_bytes!("../../../tests/fixtures/partitions/x4-stock.bin"),
+            "the stock table of the X4, byte for byte"
+        );
+        // gen_esp32part.py v5.5.2 on the X3 layout with the X4 names (SB §F, measured).
+        let x3 = builtin("x3").unwrap();
+        assert_eq!(
+            firmware::bootloader::sha256_hex(&x3.partitions.to_bytes(FlashSize::Mb16).unwrap()),
+            "334c9553e2799107ed33eb9057f8661c6dd757c662a661edac6c73bb99ae7d5e"
+        );
+        for id in builtin_ids() {
+            let profile = builtin(id).unwrap();
+            assert_eq!(profile.bootloader, &firmware::SHIPPED[0], "{id}");
+            let slot = profile.partitions.application_slot().unwrap();
+            assert_eq!(
+                (slot.name.as_str(), slot.offset),
+                ("app0", 0x1_0000),
+                "{id}"
+            );
+            assert_eq!(
+                profile.firmware_device(),
+                Device {
+                    chip: ChipId::Esp32C3,
+                    chip_revision: 3,
+                    efuse_block_revision: 102,
+                    flash_size: FlashSize::Mb16
+                },
+                "{id}"
+            );
+        }
+    }
+
+    #[test]
+    fn a_chip_family_without_a_shipped_bootloader_is_refused_by_name() {
+        let err = select_bootloader("demo.toml", ChipId::Esp32S3).unwrap_err();
+        assert_eq!(
+            err,
+            ProfileError::NoBootloader {
+                origin: "demo.toml".into(),
+                chip: ChipId::Esp32S3
+            }
+        );
+        assert!(err.to_string().contains("ESP32-S3"), "{err}");
+        assert!(select_bootloader("demo.toml", ChipId::Esp32C3).is_ok());
+    }
+
+    #[test]
+    fn a_partition_row_is_read_with_its_subtype_and_flags() {
+        let text = VALID
+            .replace("subtype = \"factory\"", "subtype = \"ota_15\"")
+            .replace(
+                "size = 0x10000\n",
+                "size = 0x10000\nencrypted = true\nreadonly = true\n",
+            );
+        let profile = DeviceProfile::from_toml("demo.toml", &text).unwrap();
+        let entry = &profile.partitions.entries[0];
+        assert_eq!(
+            (entry.ptype, entry.subtype, entry.flags),
+            (TYPE_APP, 0x1F, 0b11)
+        );
+        for (kind, subtype, value) in [
+            ("app", "factory", 0x00),
+            ("app", "test", 0x20),
+            ("app", "ota_0", 0x10),
+            ("data", "nvs", 0x02),
+            ("data", "littlefs", 0x83),
+        ] {
+            assert_eq!(
+                subtype_value(
+                    if kind == "app" {
+                        PartitionKind::App
+                    } else {
+                        PartitionKind::Data
+                    },
+                    subtype
+                ),
+                Some(value),
+                "{subtype}"
+            );
+        }
+    }
+
+    #[test]
+    fn an_unknown_subtype_is_refused_with_its_line_and_the_known_ones() {
+        for bad in ["ota_16", "ota_07", "nvs", "ota-1", ""] {
+            let err =
+                error_for(&VALID.replace("subtype = \"factory\"", &format!("subtype = \"{bad}\"")));
+            assert!(
+                err.contains("line 12")
+                    && err.contains("is not one of an app partition; write one of factory, test, or ota_0 to ota_15"),
+                "{bad}: {err}"
+            );
+        }
+        let err = error_for(
+            &VALID
+                .replace("type = \"app\"", "type = \"data\"")
+                .replace("subtype = \"factory\"", "subtype = \"ota_0\""),
+        );
+        assert!(
+            err.contains("write one of ota, phy, nvs, coredump"),
+            "{err}"
+        );
+        let err = error_for(&VALID.replace("type = \"app\"", "type = \"bootloader\""));
+        assert!(err.contains("line 14") && err.contains("`app`"), "{err}");
+    }
+
+    #[test]
+    fn a_partition_table_that_does_not_fit_the_flash_is_refused_at_load() {
+        let beyond = VALID.replace("size = 0x10000", "size = 0x400000");
+        assert!(matches!(
+            DeviceProfile::from_toml("demo.toml", &beyond),
+            Err(ProfileError::Partitions {
+                source: PartitionError::BeyondFlash { .. },
+                ..
+            })
+        ));
+        let misaligned = VALID.replace("offset = 0x10000", "offset = 0x11000");
+        let err = DeviceProfile::from_toml("demo.toml", &misaligned).unwrap_err();
+        assert!(
+            matches!(
+                err,
+                ProfileError::Partitions {
+                    source: PartitionError::Misaligned { .. },
+                    ..
+                }
+            ),
+            "{err}"
+        );
+        assert!(
+            err.to_string().starts_with("demo.toml: the partition table of the profile does not fit its flash: partition `app` starts at 0x11000"),
+            "{err}"
+        );
+        let overlap = format!(
+            "{VALID}\n[[partition]]\nname = \"nvs\"\ntype = \"data\"\nsubtype = \"nvs\"\noffset = 0x18000\nsize = 0x1000\n"
+        );
+        assert!(matches!(
+            DeviceProfile::from_toml("demo.toml", &overlap),
+            Err(ProfileError::Partitions {
+                source: PartitionError::Overlap { .. },
+                ..
+            })
+        ));
+        let none = VALID.split("\n[[partition]]").next().unwrap();
+        assert!(matches!(
+            DeviceProfile::from_toml("demo.toml", none),
+            Err(ProfileError::Partitions {
+                source: PartitionError::Empty,
+                ..
+            })
+        ));
+    }
+
+    #[test]
+    fn every_modelled_flash_size_has_a_header_code() {
+        for mib in [1, 2, 4, 8, 16] {
+            let config = FlashConfig::new(
+                mib * MIB,
+                JedecId::from_u24(0xEF_4000 | (mib.trailing_zeros() + 0x14)).unwrap(),
+            )
+            .unwrap();
+            assert_eq!(header_flash_size(config).megabytes(), mib);
+        }
+    }
+
+    #[test]
+    fn a_profile_composes_its_flash_with_its_bootloader_or_a_given_one() {
+        use firmware::builder::{ImageSpec, bootloader_spec, build_image};
+        let x4 = builtin("x4").unwrap();
+        let app = build_image(&ImageSpec::default());
+        let flash = x4.compose_flash(&app, None).unwrap();
+        assert_eq!(
+            &flash.bytes[..x4.bootloader.bytes.len()],
+            x4.bootloader.bytes
+        );
+        assert_eq!(&flash.bytes[0x1_0000..0x1_0000 + app.len()], &app[..]);
+        let given = build_image(&bootloader_spec());
+        let flash = x4.compose_flash(&app, Some(&given)).unwrap();
+        assert_eq!(&flash.bytes[..given.len()], &given[..]);
+        // A chip below the minimum of the shipped bootloader: refused, unless a bootloader that
+        // accepts it is given.
+        let old = DeviceProfile::from_toml("demo.toml", &VALID.replace("v1.1", "v0.2")).unwrap();
+        let app = build_image(&ImageSpec {
+            flash_size_nibble: FlashSize::Mb4.nibble(),
+            ..ImageSpec::default()
+        });
+        assert!(matches!(
+            old.compose_flash(&app, None),
+            Err(ComposeError::ShippedNeedsLaterChip { .. })
+        ));
+        let given = build_image(&ImageSpec {
+            flash_size_nibble: FlashSize::Mb4.nibble(),
+            ..bootloader_spec()
+        });
+        assert!(old.compose_flash(&app, Some(&given)).is_ok());
+    }
+
+    #[test]
     fn unknown_device_lists_the_known_ones() {
         let err = builtin("x5").unwrap_err().to_string();
         assert!(
@@ -439,7 +639,7 @@
     fn unknown_key_is_rejected_rather_than_ignored() {
         let err = error_for(&format!("{VALID}flash_size = \"16MB\"\n"));
         assert!(
-            err.contains("flash_size") && err.contains("line 11"),
+            err.contains("flash_size") && err.contains("line 18"),
             "{err}"
         );
     }
````


````diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -236,6 +236,13 @@
             (1, 2),
             "eFuse block revision v1.2"
         );
+        for chip in [ChipRevision::V0_3, ChipRevision::new(1, 1).unwrap()] {
+            assert_eq!(
+                Efuse::new(chip).block_revision(),
+                102,
+                "{chip}: the block revision does not follow the chip revision"
+            );
+        }
         let mac_low = word(ctx, &mut efuse, "RD_MAC_SPI_SYS_0").to_be_bytes();
         let mac_high = word(ctx, &mut efuse, "RD_MAC_SPI_SYS_1").to_be_bytes();
         assert_eq!(
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p boards --lib > /tmp/t7-red.log 2>&1; echo "exit=$?"; grep -m2 "^error" /tmp/t7-red.log
cargo test -p soc-esp32c3 --lib efuse > /tmp/t7-red2.log 2>&1; echo "exit=$?"; grep -m1 "^error" /tmp/t7-red2.log
```

Expected: `exit=101` twice; ``cannot find module or crate `firmware` in this scope`` (boards), ``no method named `block_revision` found for struct `efuse::Efuse` `` (soc-esp32c3).

- [ ] **Step 3: The eFuse block revision, the rows, the selection, the CLI and the CI**

````diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -22,10 +22,11 @@
       - run: cargo clippy --workspace --all-targets --all-features -- -D warnings
       - run: cargo test --workspace
       - run: cargo run -p svd-gen -- --check
-      # The model crates do no host I/O and must go on building for `wasm32-unknown-unknown`,
-      # the target of the web version: nothing else would notice a host dependency slipping in.
+      # The model crates, and the crates a front end composes a flash with, do no host I/O and must
+      # go on building for `wasm32-unknown-unknown`, the target of the web version: nothing else
+      # would notice a host dependency slipping in.
       - name: Build the model for WebAssembly
-        run: cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3
+        run: cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 -p firmware -p boards
       - uses: EmbarkStudios/cargo-deny-action@v2
   coverage:
     runs-on: ubuntu-24.04
````


````diff
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -122,6 +122,7 @@
 name = "boards"
 version = "0.1.0"
 dependencies = [
+ "firmware",
  "serde",
  "soc-esp32c3",
  "thiserror",
````


````diff
--- a/crates/boards/Cargo.toml
+++ b/crates/boards/Cargo.toml
@@ -10,9 +10,11 @@
 
 [dependencies]
 soc-esp32c3.workspace = true
+firmware.workspace = true
 serde.workspace = true
 toml.workspace = true
 thiserror.workspace = true
 
 [dev-dependencies]
 soc-esp32c3 = { workspace = true, features = ["test-support"] }
+firmware = { workspace = true, features = ["test-support"] }
````


````diff
--- a/crates/boards/profiles/x3.toml
+++ b/crates/boards/profiles/x3.toml
@@ -18,3 +18,53 @@
 # (docs/research/esp32c3-bootloader-accesses.md, section 2.1 item 5). `esptool.py flash_id` on a
 # unit settles it.
 jedec_id = 0x9D6018
+
+# The partition table the device boots with: `inkemu run --firmware` writes it at 0x8000 and puts
+# the application in the slot the bootloader starts on a blank otadata (the factory application,
+# else ota_0). Offsets and sizes are those of real X3 units: xteink-flasher refuses to flash an X3
+# whose table differs in type, subtype, offset or size
+# (docs/research/shipped-bootloader-and-firmware-composition.md, section F).
+# ASSUMPTION: the names, the flags and the MD5 entry of the X3 table are not established; the names
+# are those of the X4 table, no flag is set, and the table ends with its MD5 entry as
+# gen_esp32part.py writes it. A dump of the first 0x9000 bytes of a unit settles it.
+[[partition]]
+name = "nvs"
+type = "data"
+subtype = "nvs"
+offset = 0x9000
+size = 0x5000
+
+[[partition]]
+name = "otadata"
+type = "data"
+subtype = "ota"
+offset = 0xE000
+size = 0x2000
+
+[[partition]]
+name = "app0"
+type = "app"
+subtype = "ota_0"
+offset = 0x10000
+size = 0x770000
+
+[[partition]]
+name = "app1"
+type = "app"
+subtype = "ota_1"
+offset = 0x780000
+size = 0x770000
+
+[[partition]]
+name = "spiffs"
+type = "data"
+subtype = "spiffs"
+offset = 0xEF0000
+size = 0x100000
+
+[[partition]]
+name = "coredump"
+type = "data"
+subtype = "coredump"
+offset = 0xFF0000
+size = 0x10000
````


````diff
--- a/crates/boards/profiles/x4.toml
+++ b/crates/boards/profiles/x4.toml
@@ -18,3 +18,50 @@
 # (docs/research/esp32c3-bootloader-accesses.md, section 2.1 item 5). `esptool.py flash_id` on a
 # unit settles it.
 jedec_id = 0x9D6018
+
+# The partition table the device boots with: `inkemu run --firmware` writes it at 0x8000 and puts
+# the application in the slot the bootloader starts on a blank otadata (the factory application,
+# else ota_0). The stock table of the X4, compiled byte for byte as
+# tests/fixtures/partitions/x4-stock.bin; CrossPoint's partitions.csv is the same table
+# (docs/research/flash-writes-and-crosspoint-boot.md, section J).
+[[partition]]
+name = "nvs"
+type = "data"
+subtype = "nvs"
+offset = 0x9000
+size = 0x5000
+
+[[partition]]
+name = "otadata"
+type = "data"
+subtype = "ota"
+offset = 0xE000
+size = 0x2000
+
+[[partition]]
+name = "app0"
+type = "app"
+subtype = "ota_0"
+offset = 0x10000
+size = 0x640000
+
+[[partition]]
+name = "app1"
+type = "app"
+subtype = "ota_1"
+offset = 0x650000
+size = 0x640000
+
+[[partition]]
+name = "spiffs"
+type = "data"
+subtype = "spiffs"
+offset = 0xC90000
+size = 0x360000
+
+[[partition]]
+name = "coredump"
+type = "data"
+subtype = "coredump"
+offset = 0xFF0000
+size = 0x10000
````


````diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -1,9 +1,13 @@
 //! Device profiles: what a board is made of, as data.
 #![cfg_attr(test, allow(clippy::unwrap_used))]
+use firmware::bootloader::{ShippedBootloader, shipped};
+use firmware::partition::{Partition, PartitionError, PartitionTable, TYPE_APP, TYPE_DATA};
+use firmware::{Bootloader, ChipId, ComposeError, ComposedFlash, Device, FlashSize, SpiMode};
 use serde::Deserialize;
 use soc_esp32c3::flash::{
     FlashConfig, FlashConfigError, FlashImageError, JedecId, MAX_FLASH_BYTES, MIB, MIN_FLASH_BYTES,
 };
+use soc_esp32c3::periph::Efuse;
 use soc_esp32c3::{ChipRevision, Esp32C3, RomImage, SocConfig};
 use std::fmt;
 
@@ -31,6 +35,22 @@
     Esp32C3,
 }
 
+impl SocFamily {
+    /// The chip id the images of this family declare in their header. The `match` is exhaustive
+    /// on purpose: a new family does not build until it has its chip id.
+    #[must_use]
+    pub const fn chip_id(self) -> ChipId {
+        match self {
+            SocFamily::Esp32C3 => ChipId::Esp32C3,
+        }
+    }
+}
+
+/// The flash-mode family of every profile: the flash of each Xteink file seen is driven in DIO
+/// (`docs/research/shipped-bootloader-and-firmware-composition.md`, section B). A device whose
+/// flash needs QIO, or octal, adds a key to `[flash]` and a bootloader built for it.
+const FLASH_MODE: SpiMode = SpiMode::Dio;
+
 #[derive(Clone, Debug, PartialEq, Eq)]
 pub struct DeviceProfile {
     pub id: String,
@@ -38,6 +58,11 @@
     pub soc: SocFamily,
     pub chip_revision: ChipRevision,
     pub flash: FlashConfig,
+    /// The partition table an application is booted with, checked against the flash when the
+    /// profile is loaded.
+    pub partitions: PartitionTable,
+    /// The bootloader inkemu ships for this device, selected by its chip family.
+    pub bootloader: &'static ShippedBootloader,
 }
 
 /// Run-time choices that are not properties of the device.
@@ -67,6 +92,15 @@
         expected: String,
         found: String,
     },
+    #[error("{origin}: the partition table of the profile does not fit its flash: {source}")]
+    Partitions {
+        origin: String,
+        source: PartitionError,
+    },
+    #[error(
+        "{origin}: inkemu ships no bootloader for {chip} with a flash driven in DIO; choose a chip family inkemu ships a bootloader for"
+    )]
+    NoBootloader { origin: String, chip: ChipId },
 }
 
 #[derive(Deserialize)]
@@ -78,6 +112,118 @@
     soc: RawSoc,
     /// Optional here so that its absence gets a message that names the table and its keys.
     flash: Option<RawFlash>,
+    /// The `[[partition]]` rows, in the order of the table. None at all is refused as an empty
+    /// table, with the remedy of [`PartitionError::Empty`].
+    #[serde(default)]
+    partition: Vec<RawPartition>,
+}
+
+/// One `[[partition]]` row as written: `name`, `type`, `subtype`, `offset`, `size`, and the two
+/// flags of `gen_esp32part.py`, `encrypted` and `readonly`.
+#[derive(Deserialize)]
+#[serde(deny_unknown_fields)]
+struct UncheckedPartition {
+    name: String,
+    #[serde(rename = "type")]
+    kind: PartitionKind,
+    subtype: String,
+    offset: u32,
+    size: u32,
+    #[serde(default)]
+    encrypted: bool,
+    #[serde(default)]
+    readonly: bool,
+}
+
+#[derive(Clone, Copy, Deserialize)]
+#[serde(rename_all = "lowercase")]
+enum PartitionKind {
+    App,
+    Data,
+}
+
+/// The subtypes `gen_esp32part.py` (ESP-IDF v5.5.2) knows by name, with their values. The 16 OTA
+/// slots of an application are `ota_0` to `ota_15`.
+const APP_SUBTYPES: [(&str, u8); 2] = [("factory", 0x00), ("test", 0x20)];
+const APP_OTA_FIRST: u8 = 0x10;
+const APP_OTA_SLOTS: u8 = 16;
+const DATA_SUBTYPES: [(&str, u8); 11] = [
+    ("ota", 0x00),
+    ("phy", 0x01),
+    ("nvs", 0x02),
+    ("coredump", 0x03),
+    ("nvs_keys", 0x04),
+    ("efuse", 0x05),
+    ("undefined", 0x06),
+    ("esphttpd", 0x80),
+    ("fat", 0x81),
+    ("spiffs", 0x82),
+    ("littlefs", 0x83),
+];
+/// Flag bits of a partition entry (`gen_esp32part.py`).
+const FLAG_ENCRYPTED: u32 = 1 << 0;
+const FLAG_READONLY: u32 = 1 << 1;
+
+fn subtype_value(kind: PartitionKind, name: &str) -> Option<u8> {
+    match kind {
+        PartitionKind::App => APP_SUBTYPES
+            .iter()
+            .find(|(known, _)| *known == name)
+            .map(|&(_, value)| value)
+            .or_else(|| {
+                name.strip_prefix("ota_")
+                    .and_then(|slot| slot.parse::<u8>().ok())
+                    .filter(|&slot| slot < APP_OTA_SLOTS && name == format!("ota_{slot}"))
+                    .map(|slot| APP_OTA_FIRST + slot)
+            }),
+        PartitionKind::Data => DATA_SUBTYPES
+            .iter()
+            .find(|(known, _)| *known == name)
+            .map(|&(_, value)| value),
+    }
+}
+
+/// A `[[partition]]` row whose subtype is one its type has.
+#[derive(Deserialize)]
+#[serde(try_from = "UncheckedPartition")]
+struct RawPartition(Partition);
+
+impl TryFrom<UncheckedPartition> for RawPartition {
+    type Error = String;
+
+    fn try_from(raw: UncheckedPartition) -> Result<RawPartition, String> {
+        let subtype = subtype_value(raw.kind, &raw.subtype).ok_or_else(|| {
+            let (kind, known) = match raw.kind {
+                PartitionKind::App => ("app", "factory, test, or ota_0 to ota_15".to_owned()),
+                PartitionKind::Data => (
+                    "data",
+                    DATA_SUBTYPES
+                        .iter()
+                        .map(|(name, _)| *name)
+                        .collect::<Vec<_>>()
+                        .join(", "),
+                ),
+            };
+            format!(
+                "the subtype `{}` is not one of an {kind} partition; write one of {known}",
+                raw.subtype.escape_debug()
+            )
+        })?;
+        let ptype = match raw.kind {
+            PartitionKind::App => TYPE_APP,
+            PartitionKind::Data => TYPE_DATA,
+        };
+        let flags = (if raw.encrypted { FLAG_ENCRYPTED } else { 0 })
+            | (if raw.readonly { FLAG_READONLY } else { 0 });
+        Ok(RawPartition(Partition {
+            name: raw.name,
+            ptype,
+            subtype,
+            offset: raw.offset,
+            size: raw.size,
+            flags,
+        }))
+    }
 }
 
 #[derive(Deserialize)]
@@ -227,6 +373,27 @@
         .map_err(serde::de::Error::custom)
 }
 
+/// The size of `flash` as an image header states it.
+fn header_flash_size(flash: FlashConfig) -> FlashSize {
+    let Some(size) = FlashSize::from_bytes(u64::from(flash.size())) else {
+        // INVARIANT: a `FlashConfig` is 1 to 16 MiB, a power of two, each of which a header can
+        // state (a test walks every valid size).
+        unreachable!("every modelled flash size has a header code")
+    };
+    size
+}
+
+/// The bootloader inkemu ships for `chip`, in the flash-mode family of every profile.
+fn select_bootloader(
+    origin: &str,
+    chip: ChipId,
+) -> Result<&'static ShippedBootloader, ProfileError> {
+    shipped(chip, FLASH_MODE).ok_or_else(|| ProfileError::NoBootloader {
+        origin: origin.to_owned(),
+        chip,
+    })
+}
+
 /// What a refusal of the TOML says, with one remedy. The deserializers of this crate end their
 /// message with a remedy of their own (`…; write size_mib as …`); the parser's messages (an unknown
 /// key, a missing one, broken TOML) have none and get the general one. The message is the last
@@ -274,12 +441,26 @@
         let RawFlash(flash) = raw.flash.ok_or_else(|| ProfileError::MissingFlash {
             origin: origin.to_owned(),
         })?;
+        let partitions = PartitionTable {
+            entries: raw.partition.into_iter().map(|RawPartition(p)| p).collect(),
+        };
+        // The table is compiled once, to be refused here with the profile it belongs to rather
+        // than when a firmware is composed.
+        partitions
+            .to_bytes(header_flash_size(flash))
+            .map_err(|source| ProfileError::Partitions {
+                origin: origin.to_owned(),
+                source,
+            })?;
+        let bootloader = select_bootloader(origin, raw.soc.family.chip_id())?;
         Ok(DeviceProfile {
             id: raw.id,
             name: raw.name,
             soc: raw.soc.family,
             chip_revision: raw.soc.revision,
             flash,
+            partitions,
+            bootloader,
         })
     }
 
@@ -303,6 +484,38 @@
                 found: profile.id,
             })
         }
+    }
+
+    /// The device as the checks of a bootloader and an application see it: its chip, its
+    /// revision, the eFuse block revision its chip reports, its flash size.
+    #[must_use]
+    pub fn firmware_device(&self) -> Device {
+        Device {
+            chip: self.soc.chip_id(),
+            chip_revision: self.chip_revision.full(),
+            efuse_block_revision: Efuse::new(self.chip_revision).block_revision(),
+            flash_size: header_flash_size(self.flash),
+        }
+    }
+
+    /// The flash this device boots `application` from: the bootloader inkemu ships for it (or
+    /// `bootloader`, when one is given), the partition table of the profile, the application in
+    /// the slot the bootloader starts, and nothing else (`firmware::compose`).
+    ///
+    /// # Errors
+    /// [`ComposeError`] when the application or the bootloader is refused.
+    pub fn compose_flash(
+        &self,
+        application: &[u8],
+        bootloader: Option<&[u8]>,
+    ) -> Result<ComposedFlash, ComposeError> {
+        let bootloader = bootloader.map_or(Bootloader::Shipped(self.bootloader), Bootloader::Given);
+        firmware::compose(
+            &self.firmware_device(),
+            &self.partitions,
+            application,
+            bootloader,
+        )
     }
 
     #[must_use]
````


````diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -1,5 +1,5 @@
 //! inkemu command-line interface.
-use boards::{DeviceProfile, MachineOptions, SocFamily};
+use boards::{DeviceProfile, MachineOptions};
 use clap::{Parser, Subcommand};
 use cpu_rv32::{HartConfig, MisalignedPolicy};
 use emu_cli::exit::{EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
@@ -11,7 +11,6 @@
 use emu_cli::run::{Interrupt, RunOptions, execute, parse_device_seconds, relay};
 use emu_cli::rvtest::{RunResult, TestMachine};
 use emu_cli::term::sanitize;
-use firmware::ChipId;
 use soc_esp32c3::ConsoleChannel;
 use std::fmt::Write as _;
 use std::io;
@@ -249,16 +248,10 @@
         },
         other @ (boards::ProfileError::Parse { .. }
         | boards::ProfileError::MissingFlash { .. }
-        | boards::ProfileError::IdMismatch { .. }) => other,
+        | boards::ProfileError::IdMismatch { .. }
+        | boards::ProfileError::Partitions { .. }
+        | boards::ProfileError::NoBootloader { .. }) => other,
     })
-}
-
-/// The chip id a firmware image must declare to run on the device of `profile`. The `match` is
-/// exhaustive on purpose: a new chip family does not compile until it has its chip id here.
-const fn image_chip(profile: &DeviceProfile) -> ChipId {
-    match profile.soc {
-        SocFamily::Esp32C3 => ChipId::Esp32C3,
-    }
 }
 
 /// What `inkemu run` was told about the flash of the device.
@@ -300,7 +293,7 @@
         return fail(&e);
     }
     let image = match flash.image {
-        Some(path) => match flash_file::load(path, image_chip(&profile), flash_size) {
+        Some(path) => match flash_file::load(path, profile.soc.chip_id(), flash_size) {
             Ok(bytes) => Some(bytes),
             Err(e) => return fail(&e),
         },
````


````diff
--- a/crates/firmware/src/partition.rs
+++ b/crates/firmware/src/partition.rs
@@ -52,7 +52,7 @@
     pub entries: Vec<Partition>,
 }
 
-#[derive(Debug, Error, PartialEq, Eq)]
+#[derive(Clone, Debug, Error, PartialEq, Eq)]
 pub enum PartitionError {
     #[error("the partition table is empty; add at least one partition")]
     Empty,
````


````diff
--- a/crates/soc-esp32c3/src/periph/efuse.rs
+++ b/crates/soc-esp32c3/src/periph/efuse.rs
@@ -41,6 +41,15 @@
 /// The minor revision is 4 bits: its top bit lives in `RD_MAC_SPI_SYS_5`, the low three in `_3`.
 const WAFER_MINOR_LO_BITS: u32 = 3;
 const WAFER_MAJOR_SHIFT: u32 = 24;
+/// `RD_MAC_SPI_SYS_3` bits [26:24]: `BLK_VERSION_MINOR`; `RD_SYS_PART1_DATA4` bits [1:0]:
+/// `BLK_VERSION_MAJOR` (ESP-IDF v5.5.2 `hal/esp32c3/include/hal/efuse_ll.h`,
+/// `efuse_ll_get_blk_version_minor`/`_major`).
+const BLK_MINOR_SHIFT: u32 = 24;
+const BLK_MINOR_MASK: u32 = 0x7;
+const RD_SYS_PART1_DATA4: u32 = reg("RD_SYS_PART1_DATA4");
+const BLK_MAJOR_MASK: u32 = 0x3;
+/// `efuse_hal_blk_version()` is `major * 100 + minor`.
+const BLK_MAJOR_WEIGHT: u32 = 100;
 /// The wafer-version fields the profile's chip revision overrides.
 const WAFER_FIELDS_SYS_3: u32 = WAFER_MINOR_LO_MASK << WAFER_MINOR_LO_SHIFT;
 const WAFER_FIELDS_SYS_5: u32 = (0x3 << WAFER_MAJOR_SHIFT) | (1 << WAFER_MINOR_HI_SHIFT);
@@ -66,7 +75,7 @@
     (reg("RD_SYS_PART1_DATA1"), 0x6F41_FDC3),
     (reg("RD_SYS_PART1_DATA2"), 0x512C_EDBE),
     (reg("RD_SYS_PART1_DATA3"), 0x217E_E31D),
-    (reg("RD_SYS_PART1_DATA4"), 0xD864_EA41),
+    (RD_SYS_PART1_DATA4, 0xD864_EA41),
     (reg("RD_SYS_PART1_DATA5"), 0x5ABA_3A86),
     (reg("RD_SYS_PART1_DATA6"), 0x1E26_0363),
     (reg("RD_SYS_PART1_DATA7"), 0x0000_0009),
@@ -116,6 +125,23 @@
         Efuse { read_mirror }
     }
 
+    /// The eFuse block revision the chip reports, `major * 100 + minor`, as
+    /// `efuse_hal_blk_version()` computes it: what the 2nd-stage bootloader compares with the
+    /// bounds of an application's descriptor (`bootloader_common_check_efuse_blk_validity`).
+    #[must_use]
+    pub fn block_revision(&self) -> u16 {
+        let word = |at: u32| {
+            self.read_mirror
+                .iter()
+                .find(|&&(offset, _)| offset == at)
+                .map_or(0, |&(_, value)| value)
+        };
+        let minor = (word(RD_MAC_SPI_SYS_3) >> BLK_MINOR_SHIFT) & BLK_MINOR_MASK;
+        let major = word(RD_SYS_PART1_DATA4) & BLK_MAJOR_MASK;
+        // INVARIANT: 2 bits and 3 bits: at most 3 * 100 + 7, which a u16 holds.
+        u16::try_from(major * BLK_MAJOR_WEIGHT + minor).unwrap_or(u16::MAX)
+    }
+
     /// `WDT_DELAY_SEL`, `RD_REPEAT_DATA1[17:16]`: the multiplier of the stage 0 timeout of the RTC
     /// watchdog (`IDF:hal/esp32c3/include/hal/rwdt_ll.h:119`; report TW §C.3).
     #[must_use]
````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p boards --lib > /tmp/t7.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t7.log
cargo test -p soc-esp32c3 --lib efuse > /tmp/t7b.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t7b.log
cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 -p firmware -p boards > /tmp/t7-wasm.log 2>&1; echo "exit=$?"
cargo deny check > /tmp/t7-deny.log 2>&1; echo "exit=$?"
cargo test --workspace > /tmp/t7-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t7-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `27 passed`; `exit=0`, `35 passed`; the WebAssembly check `exit=0`; `cargo deny check` `exit=0`; then `exit=0` and `1052 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test -p boards --lib (the last row: cargo test -p soc-esp32c3 --lib efuse)`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `boards/src/lib.rs`: `.filter(\|&slot\| slot < APP_OTA_SLOTS && name == format!("ota_{slot}"))` → `.filter(\|&slot\| slot < APP_OTA_SLOTS)` | `an_unknown_subtype_is_refused_with_its_line_and_the_known_ones` |
| 2 | `slot < APP_OTA_SLOTS` → `slot <= APP_OTA_SLOTS` | `an_unknown_subtype_is_refused_with_its_line_and_the_known_ones` |
| 3 | delete the `partitions.to_bytes(header_flash_size(flash)).map_err(…)?;` statement of `from_toml` | `a_partition_table_that_does_not_fit_the_flash_is_refused_at_load` |
| 4 | `const FLAG_READONLY: u32 = 1 << 1;` → `1 << 2` | `a_partition_row_is_read_with_its_subtype_and_flags` |
| 5 | `bootloader.map_or(Bootloader::Shipped(self.bootloader), Bootloader::Given)` → `Bootloader::Shipped(self.bootloader)` | `a_profile_composes_its_flash_with_its_bootloader_or_a_given_one` |
| 6 | `efuse.rs`: `const BLK_MINOR_SHIFT: u32 = 24;` → `18` | `the_fields_the_boot_chain_decodes_have_the_values_it_needs` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "feat(boards): a profile states its partition table and selects the bootloader inkemu ships for its chip" -- .github/workflows/ci.yml Cargo.lock crates/boards/Cargo.toml crates/boards/profiles/x3.toml crates/boards/profiles/x4.toml crates/boards/src/lib.rs crates/emu-cli/src/main.rs crates/firmware/src/partition.rs crates/soc-esp32c3/src/periph/efuse.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 8: `inkemu licenses` prints the notices of what the binary carries, from a library a front end can call

Once the bootloader bytes are in the distributed binary, Apache-2.0 §4 asks for its licence and notices to travel with it (SB §A.3), and the BSD-3-Clause clause 2 for the two RISC-V notices. `boards::notices` holds the root `NOTICE` and, for each third-party part the binary carries (the register descriptions compiled into `svd-regs`, the bootloader embedded by `firmware`), its notice and its licence, built in with `include_str!`; `inkemu licenses` prints them as one text. A front end shows the same text on its "About" page. The licences of the Rust crates linked into the binary go to SP9 (C24).

**Files:**
- Modify: `crates/boards/src/lib.rs`
- Create: `crates/boards/src/notices.rs`
- Modify: `crates/emu-cli/src/main.rs`
- Create: `crates/emu-cli/tests/licenses_cli.rs`

**Interfaces:**
- Consumes: the files of `third_party/espressif-svd/` and `third_party/esp-idf-bootloader/`, the root `NOTICE`.
- Produces: `boards::notices::{NOTICE: &str, ThirdParty { name, directory, notice, license }, THIRD_PARTIES: [ThirdParty; 2], notices_text() -> String}`; the subcommand `inkemu licenses`.

**Bounds on untrusted input.** None: the texts are built into the binary.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/boards/src/lib.rs
+++ b/crates/boards/src/lib.rs
@@ -1,5 +1,7 @@
 //! Device profiles: what a board is made of, as data.
 #![cfg_attr(test, allow(clippy::unwrap_used))]
+pub mod notices;
+
 use firmware::bootloader::{ShippedBootloader, shipped};
 use firmware::partition::{Partition, PartitionError, PartitionTable, TYPE_APP, TYPE_DATA};
 use firmware::{Bootloader, ChipId, ComposeError, ComposedFlash, Device, FlashSize, SpiMode};
````


Create `crates/boards/src/notices.rs` with its tests only:

````rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_of_inkemu_names_every_part_and_every_part_carries_its_texts() {
        for part in &THIRD_PARTIES {
            assert!(
                NOTICE.contains(&format!("See {}/.", part.directory)),
                "the NOTICE has no paragraph for {}",
                part.directory
            );
            assert!(part.notice.contains("Espressif"), "{}", part.directory);
            assert!(
                part.license.contains("Apache License") && part.license.contains("Version 2.0"),
                "{}",
                part.directory
            );
        }
    }

    #[test]
    fn the_text_holds_every_notice_and_licence() {
        let text = notices_text();
        assert!(text.starts_with("inkemu\n"), "{text}");
        for part in &THIRD_PARTIES {
            assert!(text.contains(part.notice.trim_end()), "{}", part.directory);
            assert!(text.contains(part.license.trim_end()), "{}", part.directory);
        }
        assert!(
            text.contains("The Regents of the University of California"),
            "the BSD-3-Clause notice of the bootloader"
        );
    }
}
````


Create `crates/emu-cli/tests/licenses_cli.rs`:

````rust
//! `inkemu licenses` through the real binary.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration test: a panic is a test failure
use assert_cmd::Command;

#[test]
fn licenses_prints_the_notice_then_every_part_with_its_licence() {
    let output = Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .arg("licenses")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(
        text,
        boards::notices::notices_text(),
        "the library's text, as it is"
    );
    for expected in [
        "This product includes the ESP-IDF 2nd-stage bootloader",
        "== ESP-IDF 2nd-stage bootloader (third_party/esp-idf-bootloader) ==",
        "== ESP32-C3 register descriptions (Espressif CMSIS-SVD) (third_party/espressif-svd) ==",
        "Copyright (c) 2018-2019, The libfemto authors",
        "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION",
    ] {
        assert!(text.contains(expected), "`{expected}` is missing");
    }
}

#[test]
fn the_help_lists_the_licenses_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_inkemu"))
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).unwrap();
    assert!(
        help.contains("licenses") && help.contains("third-party parts built into inkemu"),
        "{help}"
    );
}
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p boards --lib notices > /tmp/t8-red.log 2>&1; echo "exit=$?"; grep -m2 "^error" /tmp/t8-red.log
cargo test -p emu-cli --test licenses_cli > /tmp/t8-red2.log 2>&1; echo "exit=$?"; grep -m1 "^error" /tmp/t8-red2.log
```

Expected: `exit=101` twice; ``cannot find value `THIRD_PARTIES` in this scope``, ``cannot find value `NOTICE` in this scope``; ``cannot find function `notices_text` in module `boards::notices` ``.

- [ ] **Step 3: The notices and the command**

Put this above the tests of `crates/boards/src/notices.rs`:

````rust
//! What the inkemu binary carries from third parties, and the notices their licences ask a
//! distribution to carry with it (Apache License 2.0, section 4; BSD-3-Clause, clause 2). The
//! texts are those of the repository, built into the binary: `inkemu licenses` prints them, and a
//! front end shows them on its "About" page.
use std::fmt::Write as _;

/// The `NOTICE` of inkemu: one paragraph per third-party part.
pub const NOTICE: &str = include_str!("../../../NOTICE");

/// A third-party part of the binary.
#[derive(Debug, PartialEq, Eq)]
pub struct ThirdParty {
    /// What it is, in a few words.
    pub name: &'static str,
    /// Where the repository keeps it, with its licence and provenance.
    pub directory: &'static str,
    pub notice: &'static str,
    pub license: &'static str,
}

/// Every third-party part the binary carries. The register descriptions are compiled into the
/// model (`svd-regs`, generated from the SVD); the bootloader is embedded byte for byte
/// (`firmware::SHIPPED`).
pub static THIRD_PARTIES: [ThirdParty; 2] = [
    ThirdParty {
        name: "ESP32-C3 register descriptions (Espressif CMSIS-SVD)",
        directory: "third_party/espressif-svd",
        notice: include_str!("../../../third_party/espressif-svd/NOTICE"),
        license: include_str!("../../../third_party/espressif-svd/LICENSE"),
    },
    ThirdParty {
        name: "ESP-IDF 2nd-stage bootloader",
        directory: "third_party/esp-idf-bootloader",
        notice: include_str!("../../../third_party/esp-idf-bootloader/NOTICE"),
        license: include_str!("../../../third_party/esp-idf-bootloader/LICENSE"),
    },
];

/// Every notice and every licence text, as one text: the `NOTICE` of inkemu, then each part with
/// its own notice and licence.
#[must_use]
pub fn notices_text() -> String {
    let mut text = String::from(NOTICE.trim_end());
    text.push('\n');
    for part in &THIRD_PARTIES {
        // Writing to a String cannot fail.
        let _ = write!(
            text,
            "\n== {} ({}) ==\n\n{}\n\n-- licence --\n\n{}\n",
            part.name,
            part.directory,
            part.notice.trim_end(),
            part.license.trim_end()
        );
    }
    text
}

````


````diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -113,6 +113,9 @@
         #[arg(long)]
         trace_hash: bool,
     },
+    /// Print the notices and the licences of the third-party parts built into inkemu: the
+    /// register descriptions of the chip and the 2nd-stage bootloader it ships.
+    Licenses,
     /// Manage the mask-ROM files the emulator needs.
     Rom {
         #[command(subcommand)]
@@ -169,6 +172,10 @@
             allow_misaligned,
         ),
         Command::Inspect { file } => run_inspect(&file),
+        Command::Licenses => match print_stdout(&boards::notices::notices_text()) {
+            ControlFlow::Continue(()) => ExitCode::SUCCESS,
+            ControlFlow::Break(code) => code,
+        },
         Command::Run {
             device,
             flash,
````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p boards --lib notices > /tmp/t8.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t8.log
cargo test -p emu-cli --test licenses_cli > /tmp/t8b.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t8b.log
cargo test --workspace > /tmp/t8-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t8-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `2 passed`; `exit=0`, `2 passed`; then `exit=0` and `1056 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test -p boards --lib notices; cargo test -p emu-cli --test licenses_cli`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `NOTICE`: `See third_party/esp-idf-bootloader/.` → `See the bootloader.` | `the_notice_of_inkemu_names_every_part_and_every_part_carries_its_texts` |
| 2 | `main.rs`: `print_stdout(&boards::notices::notices_text())` → `print_stdout(boards::notices::NOTICE)` | `licenses_prints_the_notice_then_every_part_with_its_licence` |
| 3 | `notices.rs`: the last argument of `write!`, `part.license.trim_end()` → `""` | `the_text_holds_every_notice_and_licence` |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/boards/src/notices.rs crates/emu-cli/tests/licenses_cli.rs
/usr/bin/git commit -m "feat(cli): inkemu licenses prints the notices of what the binary carries, from a library a front end can call" -- crates/boards/src/lib.rs crates/boards/src/notices.rs crates/emu-cli/src/main.rs crates/emu-cli/tests/licenses_cli.rs
```

Expected: three `exit=0`, then the commit.

---

## Task 9: `inkemu run --firmware` boots an application image behind the bootloader the device ships with, or `--bootloader`

The options of the milestone. `--firmware <FILE>` (exclusive with `--flash`) and `--bootloader <FILE>` (which requires `--firmware`) are read with the bounded reader of `--flash`, now shared (`flash_file::read_regular_file`: a regular file only, refused on its metadata above the flash size, never more than one byte past it); `firmware_file::compose` hands the bytes to `DeviceProfile::compose_flash` and names the file a refusal is about (the firmware, the bootloader, or the device). The composed flash is the seed of the run, as the `--flash` image is (ADR 0004, C10): with an existing `--flash-file` it is refused unless `--reset-flash`, it cannot be the flash file or its temporary or lock file, and it is never written. The refusals of `flash_state.rs` name the option that gave the file (C28). The warnings of a given bootloader are printed before the run; the header line says what the flash is made of. ADR 0005 is written: everything it decides is now true.

**Files:**
- Create: `crates/emu-cli/src/firmware_file.rs`
- Modify: `crates/emu-cli/src/flash_file.rs`
- Modify: `crates/emu-cli/src/flash_state.rs`
- Modify: `crates/emu-cli/src/lib.rs`
- Modify: `crates/emu-cli/src/main.rs`
- Modify: `crates/emu-cli/tests/run_cli.rs`
- Create: `docs/adr/0005-bootloader-embarque.md`

**Interfaces:**
- Consumes: `DeviceProfile::compose_flash` (Task 7), `ComposeError`, `RunFlash::begin`, `refuse_the_image_as_flash_file`.
- Produces: `flash_file::{ReadFailure::{Io, NotAFile, TooLong(u64), Grew(u64)}, read_regular_file(&Path, u32) -> Result<Vec<u8>, ReadFailure>}` (crate-visible); `flash_state::{Input::{Flash, Firmware, Bootloader} (Display: the option), Seed { input, content }}`; `RunFlash::begin(Option<&Path>, bool, Option<Seed>, u32)`; `refuse_the_image_as_flash_file(Input, &Path, &Path)`; `FlashStateError::{BothGiven { path, input }, SameFile { input, image, path }, ImageIsTheRunsOwn { input, … }}`; `emu_cli::firmware_file::{FirmwareFileError, compose(&DeviceProfile, &Path, Option<&Path>) -> Result<ComposedFlash, FirmwareFileError>}`; the options `--firmware` and `--bootloader` of `inkemu run`; `docs/adr/0005-bootloader-embarque.md`.

**Bounds on untrusted input.** Each file is judged on its metadata before it is opened (a regular file, at most the flash size: 16 MiB) and read through `take(size + 1)`; a pipe is refused without being opened; the checks of Tasks 4 to 6 bound the rest.

- [ ] **Step 1: Write the failing tests**

Create `crates/emu-cli/src/firmware_file.rs` with its tests only:

````rust
#[cfg(test)]
mod tests {
    use super::*;
    use firmware::builder::{ImageSpec, bootloader_spec, build_flash_image, build_image};
    use firmware::{ApplicationError, BootloaderError};

    fn x4() -> DeviceProfile {
        boards::builtin("x4").unwrap()
    }

    fn file_with(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn an_application_is_composed_behind_the_shipped_bootloader_or_the_one_given() {
        let dir = tempfile::tempdir().unwrap();
        let app = build_image(&ImageSpec::default());
        let firmware = file_with(&dir, "app.bin", &app);
        let flash = compose(&x4(), &firmware, None).unwrap();
        assert_eq!(flash.bytes, x4().compose_flash(&app, None).unwrap().bytes);
        let given = build_image(&bootloader_spec());
        let bootloader = file_with(&dir, "bootloader.bin", &given);
        let flash = compose(&x4(), &firmware, Some(&bootloader)).unwrap();
        assert_eq!(&flash.bytes[..given.len()], &given[..]);
    }

    #[test]
    fn a_refusal_names_the_file_it_is_about() {
        let dir = tempfile::tempdir().unwrap();
        let app = file_with(&dir, "app.bin", &build_image(&ImageSpec::default()));
        let dump = file_with(&dir, "dump.bin", &build_flash_image(&ImageSpec::default()));
        let err = compose(&x4(), &dump, None).unwrap_err();
        assert!(
            matches!(
                &err,
                FirmwareFileError::Refused {
                    source: ComposeError::Application(ApplicationError::FlashImage),
                    what
                } if what.ends_with("dump.bin")
            ),
            "{err}"
        );
        assert!(
            err.to_string()
                .contains("; use it as the flash (`--flash`)"),
            "{err}"
        );
        let wrong = file_with(&dir, "wrong.bin", &build_image(&ImageSpec::default()));
        let err = compose(&x4(), &app, Some(&wrong)).unwrap_err();
        assert!(
            matches!(
                &err,
                FirmwareFileError::Refused {
                    source: ComposeError::Bootloader(BootloaderError::Application),
                    what
                } if what.ends_with("wrong.bin")
            ),
            "{err}"
        );
    }

    #[test]
    fn a_file_that_is_missing_too_long_or_no_file_is_refused_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let app = file_with(&dir, "app.bin", &build_image(&ImageSpec::default()));
        let err = compose(&x4(), &dir.path().join("none.bin"), None).unwrap_err();
        assert!(
            matches!(
                err,
                FirmwareFileError::Read {
                    input: Input::Firmware,
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("given to --firmware"), "{err}");
        let err = compose(&x4(), &app, Some(dir.path())).unwrap_err();
        assert!(
            matches!(
                err,
                FirmwareFileError::NotAFile {
                    input: Input::Bootloader,
                    ..
                }
            ),
            "{err}"
        );
        // A sparse file one byte longer than the flash: nothing that large is read.
        let big = dir.path().join("big.bin");
        std::fs::File::create(&big)
            .unwrap()
            .set_len((16 << 20) + 1)
            .unwrap();
        let err = compose(&x4(), &big, None).unwrap_err();
        assert!(
            matches!(
                err,
                FirmwareFileError::TooLong {
                    input: Input::Firmware,
                    len: 16_777_217,
                    size: 16_777_216,
                    ..
                }
            ),
            "{err}"
        );
    }
}
````


````diff
--- a/crates/emu-cli/src/flash_state.rs
+++ b/crates/emu-cli/src/flash_state.rs
@@ -1549,7 +1549,7 @@
 
         // A run that is refused, here for an image given next to a flash file that exists,
         // leaves the directory as it found it but for its lock file.
-        let err = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap_err();
+        let err = RunFlash::begin(Some(&path), false, Some(flash_seed()), MIB).unwrap_err();
         assert!(matches!(err, FlashStateError::BothGiven { .. }), "{err}");
         assert_eq!(std::fs::read(&temporary).unwrap(), LEFT, "not removed");
 
@@ -1883,9 +1883,17 @@
         vec![0x5A; 0x4000]
     }
 
+    /// [`seed`], given by `--flash`.
+    fn flash_seed() -> Seed {
+        Seed {
+            input: Input::Flash,
+            content: seed(),
+        }
+    }
+
     #[test]
     fn without_a_flash_file_the_run_starts_from_the_seed_and_says_what_it_does_not_keep() {
-        let (flash, content) = RunFlash::begin(None, false, Some(seed()), MIB).unwrap();
+        let (flash, content) = RunFlash::begin(None, false, Some(flash_seed()), MIB).unwrap();
         assert_eq!(content, Some(seed()));
         assert!(flash.state.is_none() && !flash.starts_from_the_file());
         let start = format!(
@@ -1922,7 +1930,8 @@
         let dir = tempfile::tempdir().unwrap();
         let path = dir.path().join("x4.bin");
         // First run: no file yet, the seed, a write.
-        let (flash, content) = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap();
+        let (flash, content) =
+            RunFlash::begin(Some(&path), false, Some(flash_seed()), MIB).unwrap();
         assert_eq!(content, Some(seed()));
         assert!(!flash.starts_from_the_file());
         let machine = ran(content, &writing(0x00));
@@ -1990,7 +1999,7 @@
         let path = dir.path().join("x4.bin");
         let kept_content = vec![0x11u8; MIB as usize];
         std::fs::write(&path, &kept_content).unwrap();
-        let err = RunFlash::begin(Some(&path), false, Some(seed()), MIB).unwrap_err();
+        let err = RunFlash::begin(Some(&path), false, Some(flash_seed()), MIB).unwrap_err();
         assert!(matches!(err, FlashStateError::BothGiven { .. }), "{err}");
         let text = err.to_string();
         assert!(
@@ -2004,7 +2013,7 @@
 
         // The refusal released the lock. A reset starts from the seed and replaces the file,
         // even when the run modified nothing.
-        let (flash, content) = RunFlash::begin(Some(&path), true, Some(seed()), MIB).unwrap();
+        let (flash, content) = RunFlash::begin(Some(&path), true, Some(flash_seed()), MIB).unwrap();
         assert_eq!(content, Some(seed()));
         assert!(!flash.starts_from_the_file());
         let (kept, log) = finish(&flash, &ran(content, &[]));
@@ -2036,7 +2045,7 @@
         let image = dir.path().join("x4.bin");
         std::fs::write(&image, seed()).unwrap();
         let refused = |route: &str, image: &Path, flash_file: &Path| {
-            let err = refuse_the_image_as_flash_file(image, flash_file).unwrap_err();
+            let err = refuse_the_image_as_flash_file(Input::Flash, image, flash_file).unwrap_err();
             assert!(
                 matches!(err, FlashStateError::SameFile { .. }),
                 "{route}: {err}"
@@ -2069,10 +2078,11 @@
         // Another file with the same bytes, a file that is not there yet, and none at all.
         let copy = dir.path().join("copy.bin");
         std::fs::write(&copy, seed()).unwrap();
-        refuse_the_image_as_flash_file(&image, &copy).unwrap();
-        refuse_the_image_as_flash_file(&image, &dir.path().join("new.bin")).unwrap();
-        refuse_the_image_as_flash_file(&dir.path().join("gone.bin"), &image).unwrap();
-        refuse_the_image_as_flash_file(&dir.path().join("a"), &dir.path().join("a")).unwrap();
+        refuse_the_image_as_flash_file(Input::Flash, &image, &copy).unwrap();
+        refuse_the_image_as_flash_file(Input::Flash, &image, &dir.path().join("new.bin")).unwrap();
+        refuse_the_image_as_flash_file(Input::Flash, &dir.path().join("gone.bin"), &image).unwrap();
+        refuse_the_image_as_flash_file(Input::Flash, &dir.path().join("a"), &dir.path().join("a"))
+            .unwrap();
     }
 
     #[test]
````


````diff
--- a/crates/emu-cli/src/lib.rs
+++ b/crates/emu-cli/src/lib.rs
@@ -1,5 +1,6 @@
 //! Library side of the inkemu CLI, so integration tests can drive the same code as the binary.
 pub mod exit;
+pub mod firmware_file;
 pub mod flash_file;
 pub mod flash_state;
 pub mod inspect;
````


````diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -2696,3 +2696,218 @@
     assert_eq!(third.sha256(FLASH_AT_END), sha256_of(&file));
     assert_eq!(sha256_of(&image), NVS_IMAGE_SHA256, "--flash is only read");
 }
+
+/// `inkemu run --device x4 --firmware <application> [--bootloader <bootloader>] <more>` with a ROM
+/// directory that holds nothing: a refusal of what was given must come before any attempt to load
+/// a ROM.
+fn run_with_firmware(
+    application: &[u8],
+    bootloader: Option<&[u8]>,
+    more: &[&str],
+) -> assert_cmd::assert::Assert {
+    let dir = tempfile::tempdir().unwrap();
+    let firmware = dir.path().join("app.bin");
+    std::fs::write(&firmware, application).unwrap();
+    let mut command = inkemu();
+    command
+        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
+        .arg(dir.path().join("rom"))
+        .arg("--firmware")
+        .arg(&firmware);
+    if let Some(bytes) = bootloader {
+        let path = dir.path().join("bootloader.bin");
+        std::fs::write(&path, bytes).unwrap();
+        command.arg("--bootloader").arg(path);
+    }
+    command.args(more).timeout(HANG_LIMIT).assert()
+}
+
+/// What `run_with_firmware` must refuse before boot, each with its cause and its remedy.
+#[test]
+fn every_input_that_cannot_be_booted_from_an_application_is_refused_before_boot() {
+    use firmware::builder::{bootloader_spec, refresh_digest};
+    let app = build_image(&ImageSpec::default());
+    let bootloader = build_image(&bootloader_spec());
+    let mut xota = b"XOTA".to_vec();
+    xota.resize(512, 0);
+    let s3 = build_image(&ImageSpec {
+        chip: 9,
+        ..ImageSpec::default()
+    });
+    let mut corrupt = app.clone();
+    let last = corrupt.len() - 1;
+    corrupt[last] ^= 1;
+    // A data segment the size of the X4's slot, after the descriptor: the image is larger.
+    let mut too_large = ImageSpec::default();
+    too_large.segments.push((0x3C10_0000, vec![0; 0x64_0000]));
+    let too_large = build_image(&too_large);
+    let mut later_chip = bootloader.clone();
+    later_chip[15..17].copy_from_slice(&4u16.to_le_bytes());
+    refresh_digest(&mut later_chip);
+    for (case, application, given, cause, remedy) in [
+        (
+            "not an image",
+            vec![0u8; 64],
+            None,
+            "its first byte is 0x00, expected 0xe9",
+            "; use the plain application .bin",
+        ),
+        (
+            "an encrypted update",
+            xota,
+            None,
+            "is an encrypted OTA update (.xota)",
+            "; use the plain application .bin of a release",
+        ),
+        (
+            "an S3 image",
+            s3,
+            None,
+            "is built for ESP32-S3 and the device uses ESP32-C3",
+            "; choose a device with that chip",
+        ),
+        (
+            "larger than its slot",
+            too_large,
+            None,
+            "and its slot, app0, holds 6553600",
+            "; use an application that fits",
+        ),
+        (
+            "a corrupt digest",
+            corrupt,
+            None,
+            "the firmware is damaged: SHA-256 mismatch",
+            "; re-download the file or rebuild the firmware",
+        ),
+        (
+            "a whole flash image",
+            build_flash_image(&ImageSpec::default()),
+            None,
+            "is a whole flash image",
+            "; use it as the flash (`--flash`)",
+        ),
+        (
+            "a bootloader as the firmware",
+            bootloader.clone(),
+            None,
+            "the firmware is a 2nd-stage bootloader",
+            "; give it as the bootloader (`--bootloader`)",
+        ),
+        (
+            "an application as the bootloader",
+            app.clone(),
+            Some(app.clone()),
+            "the bootloader is an application image",
+            "; use it as the firmware (`--firmware`)",
+        ),
+        (
+            "a bootloader for a later chip",
+            app.clone(),
+            Some(later_chip),
+            "requires chip revision v0.4 or later and the device is v0.3: it would reset the chip for ever",
+            "; give a bootloader built for v0.3",
+        ),
+    ] {
+        let assert = run_with_firmware(&application, given.as_deref(), &[])
+            .code(2)
+            .stdout(predicate::str::is_empty());
+        let log = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
+        assert!(
+            log.starts_with("error: ") && log.contains(cause) && log.contains(remedy),
+            "{case}: {log}"
+        );
+        assert!(
+            !log.contains(MISSING_ROM),
+            "{case}: refused before the ROM: {log}"
+        );
+    }
+}
+
+#[test]
+fn a_firmware_and_a_bootloader_that_pass_are_taken_to_the_rom() {
+    use firmware::builder::bootloader_spec;
+    // The ROM directory is empty: getting as far as the ROM is getting past every check.
+    let app = build_image(&ImageSpec::default());
+    for given in [None, Some(build_image(&bootloader_spec()))] {
+        run_with_firmware(&app, given.as_deref(), &[])
+            .code(2)
+            .stderr(contains(MISSING_ROM));
+    }
+}
+
+#[test]
+fn a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply() {
+    let app = build_image(&ImageSpec::default());
+    let dir = tempfile::tempdir().unwrap();
+    let file = dir.path().join("x4.bin");
+    sparse_file(&file, X4_FLASH_BYTES);
+    let firmware = dir.path().join("app.bin");
+    std::fs::write(&firmware, &app).unwrap();
+    let firmware_arg = firmware.to_str().unwrap();
+    run_with_flash_file(&file, &["--firmware", firmware_arg])
+        .code(2)
+        .stderr(
+            contains("both --firmware and the flash file")
+                .and(contains("; drop --firmware to go on from the flash file"))
+                .and(contains(
+                    "start again from the flash composed from the firmware",
+                ))
+                .and(contains(MISSING_ROM).not()),
+        );
+    // With --reset-flash the run goes as far as the ROM.
+    run_with_flash_file(&file, &["--firmware", firmware_arg, "--reset-flash"])
+        .code(2)
+        .stderr(contains(MISSING_ROM));
+    // The firmware cannot be the flash file, nor --bootloader.
+    run_with_flash_file(&firmware, &["--firmware", firmware_arg])
+        .code(2)
+        .stderr(contains("--firmware and --flash-file name the same file"));
+    let other = dir.path().join("other.bin");
+    run_with_flash_file(
+        &firmware,
+        &[
+            "--firmware",
+            other.to_str().unwrap(),
+            "--bootloader",
+            firmware_arg,
+        ],
+    )
+    .code(2)
+    .stderr(contains("--bootloader and --flash-file name the same file"));
+    assert_eq!(std::fs::read(&firmware).unwrap(), app, "never written");
+}
+
+#[test]
+fn a_firmware_with_an_image_or_a_bootloader_without_a_firmware_is_a_usage_error() {
+    let dir = tempfile::tempdir().unwrap();
+    let path = dir.path().join("x.bin");
+    std::fs::write(&path, b"x").unwrap();
+    let path = path.to_str().unwrap();
+    inkemu()
+        .args(["run", "--device", "x4", "--flash", path, "--firmware", path])
+        .assert()
+        .code(2)
+        .stderr(contains(
+            "'--flash <FLASH>' cannot be used with '--firmware <FILE>'",
+        ));
+    inkemu()
+        .args(["run", "--device", "x4", "--bootloader", path])
+        .assert()
+        .code(2)
+        .stderr(contains("--firmware <FILE>"));
+}
+
+#[test]
+fn run_help_documents_the_firmware_and_the_bootloader() {
+    let help = inkemu().args(["run", "--help"]).assert().success();
+    let out = String::from_utf8_lossy(&help.get_output().stdout).into_owned();
+    for expected in [
+        "--firmware <FILE>",
+        "the partition table of the device profile at 0x8000",
+        "--bootloader <FILE>",
+        "instead of the one inkemu ships",
+    ] {
+        assert!(out.contains(expected), "`{expected}` is missing:\n{out}");
+    }
+}
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p emu-cli --lib > /tmp/t9-red.log 2>&1; echo "exit=$?"; grep -m3 "^error" /tmp/t9-red.log
cargo test -p emu-cli --test run_cli > /tmp/t9-red2.log 2>&1; echo "exit=$?"; grep "FAILED\|^test result" /tmp/t9-red2.log
```

Expected: `exit=101`; the library tests do not compile (``cannot find type `DeviceProfile` in this scope``, ``cannot find type `Seed` in this scope``). Then `exit=101`; five tests fail, `41 passed; 5 failed; 28 ignored`: `every_input_that_cannot_be_booted_from_an_application_is_refused_before_boot`, `a_firmware_and_a_bootloader_that_pass_are_taken_to_the_rom`, `a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply`, `a_firmware_with_an_image_or_a_bootloader_without_a_firmware_is_a_usage_error`, `run_help_documents_the_firmware_and_the_bootloader` (`unexpected argument '--firmware' found`).

- [ ] **Step 3: The reader, the seed, the module, the options, the ADR**

Put this above the tests of `crates/emu-cli/src/firmware_file.rs`:

````rust
//! Reads the files given to `inkemu run --firmware` and `--bootloader`, and has the device
//! profile compose the flash the device boots from, before any machine exists (spec §5.1). The
//! checks and the composition belong to the profile (`boards::DeviceProfile::compose_flash`), so
//! that a front end gets the same answers; this module only reads files and names them.
use crate::flash_file::{ReadFailure, read_regular_file};
use crate::flash_state::Input;
use crate::term::sanitize;
use boards::DeviceProfile;
use firmware::{ComposeError, ComposedFlash};
use std::path::Path;

/// Why no flash can be composed from the files given. The messages state the cause, then the
/// remedy; the paths they echo have their control characters escaped.
#[derive(Debug, thiserror::Error)]
pub enum FirmwareFileError {
    #[error(
        "cannot read {path}, given to {input}: {detail}; check the path and its read permissions"
    )]
    Read {
        input: Input,
        path: String,
        detail: String,
    },
    #[error("{path}, given to {input}, is not a regular file; give the path of the file itself")]
    NotAFile { input: Input, path: String },
    #[error(
        "{path}, given to {input}, is {len} bytes, more than the {size} byte flash of the device, so it is no application nor bootloader of this device; give the .bin of a build, or a flash image of this device to --flash"
    )]
    TooLong {
        input: Input,
        path: String,
        len: u64,
        size: u32,
    },
    #[error(
        "{path}, given to {input}, holds at least {at_least} bytes (it grew while it was being read), more than the {size} byte flash of the device; give a file that nothing else writes"
    )]
    Grew {
        input: Input,
        path: String,
        at_least: u64,
        size: u32,
    },
    /// `what` is the file the refusal is about, or the device when it is about its profile.
    #[error("{what}: {source}")]
    Refused {
        what: String,
        #[source]
        source: ComposeError,
    },
}

fn shown(path: &Path) -> String {
    sanitize(&path.display().to_string())
}

/// The content of the file `path`, given to `input`: at most `limit` bytes, the flash size of the
/// device, of which no more than one more byte is read.
fn read(input: Input, path: &Path, limit: u32) -> Result<Vec<u8>, FirmwareFileError> {
    read_regular_file(path, limit).map_err(|failure| match failure {
        ReadFailure::Io(e) => FirmwareFileError::Read {
            input,
            path: shown(path),
            detail: sanitize(&e.to_string()),
        },
        ReadFailure::NotAFile => FirmwareFileError::NotAFile {
            input,
            path: shown(path),
        },
        ReadFailure::TooLong(len) => FirmwareFileError::TooLong {
            input,
            path: shown(path),
            len,
            size: limit,
        },
        ReadFailure::Grew(at_least) => FirmwareFileError::Grew {
            input,
            path: shown(path),
            at_least,
            size: limit,
        },
    })
}

/// The flash `profile` boots `firmware` from, behind `bootloader` when one is given and the
/// bootloader inkemu ships for the device otherwise.
///
/// # Errors
/// [`FirmwareFileError`]: a file cannot be read, or the profile refuses what it holds.
pub fn compose(
    profile: &DeviceProfile,
    firmware: &Path,
    bootloader: Option<&Path>,
) -> Result<ComposedFlash, FirmwareFileError> {
    let limit = profile.flash.size();
    let application = read(Input::Firmware, firmware, limit)?;
    let given = bootloader
        .map(|path| read(Input::Bootloader, path, limit))
        .transpose()?;
    profile
        .compose_flash(&application, given.as_deref())
        .map_err(|source| {
            let device = || format!("device {}", sanitize(&profile.id));
            let what = match (&source, bootloader) {
                (ComposeError::Application(_), _) => shown(firmware),
                (ComposeError::Bootloader(_), Some(path)) => shown(path),
                _ => device(),
            };
            FirmwareFileError::Refused { what, source }
        })
}

````


````diff
--- a/crates/emu-cli/src/flash_file.rs
+++ b/crates/emu-cli/src/flash_file.rs
@@ -43,6 +43,41 @@
     },
 }
 
+/// Why a file could not be read whole within a limit.
+#[derive(Debug)]
+pub(crate) enum ReadFailure {
+    Io(std::io::Error),
+    /// A directory, a pipe, a device: refused without being read.
+    NotAFile,
+    /// The file is this many bytes, more than the limit, and was not read.
+    TooLong(u64),
+    /// The file held at least this many bytes when it was read, more than the limit: it grew
+    /// after its size was looked at.
+    Grew(u64),
+}
+
+/// The content of the regular file `path`, which must hold at most `limit` bytes. A path that is
+/// not a regular file is refused without being opened (a pipe would block), and no more than
+/// `limit + 1` bytes are ever read.
+pub(crate) fn read_regular_file(path: &Path, limit: u32) -> Result<Vec<u8>, ReadFailure> {
+    let metadata = std::fs::metadata(path).map_err(ReadFailure::Io)?;
+    if !metadata.is_file() {
+        return Err(ReadFailure::NotAFile);
+    }
+    if metadata.len() > u64::from(limit) {
+        return Err(ReadFailure::TooLong(metadata.len()));
+    }
+    let file = std::fs::File::open(path).map_err(ReadFailure::Io)?;
+    // The path may have been replaced between the two calls: judge what was actually opened.
+    if !file.metadata().map_err(ReadFailure::Io)?.is_file() {
+        return Err(ReadFailure::NotAFile);
+    }
+    // One byte past the limit: a file that grew since the metadata was read is still caught.
+    read_bounded(file, limit)
+        .map_err(ReadFailure::Io)?
+        .map_err(ReadFailure::Grew)
+}
+
 /// Reads `source` to its end, but never more than one byte past `limit`: `Err(n)` when it holds at
 /// least `n` bytes, more than `limit`.
 fn read_bounded(source: impl Read, limit: u32) -> std::io::Result<Result<Vec<u8>, u64>> {
@@ -64,37 +99,25 @@
 /// [`FlashFileError`], whose message names the file, the cause and the remedy.
 pub fn load(path: &Path, chip: ChipId, flash_size: u32) -> Result<Vec<u8>, FlashFileError> {
     let shown = sanitize(&path.display().to_string());
-    let read_error = |e: &std::io::Error| FlashFileError::Read {
-        path: shown.clone(),
-        detail: sanitize(&e.to_string()),
-    };
-    let metadata = std::fs::metadata(path).map_err(|e| read_error(&e))?;
-    if !metadata.is_file() {
-        return Err(FlashFileError::NotAFile { path: shown });
-    }
-    if metadata.len() > u64::from(flash_size) {
-        return Err(FlashFileError::TooLong {
-            path: shown,
-            len: metadata.len(),
+    let bytes = read_regular_file(path, flash_size).map_err(|failure| match failure {
+        ReadFailure::Io(e) => FlashFileError::Read {
+            path: shown.clone(),
+            detail: sanitize(&e.to_string()),
+        },
+        ReadFailure::NotAFile => FlashFileError::NotAFile {
+            path: shown.clone(),
+        },
+        ReadFailure::TooLong(len) => FlashFileError::TooLong {
+            path: shown.clone(),
+            len,
             size: flash_size,
-        });
-    }
-    let file = std::fs::File::open(path).map_err(|e| read_error(&e))?;
-    // The path may have been replaced between the two calls: judge what was actually opened.
-    if !file.metadata().map_err(|e| read_error(&e))?.is_file() {
-        return Err(FlashFileError::NotAFile { path: shown });
-    }
-    // One byte past the flash: a file that grew since the metadata was read is still caught.
-    let bytes = match read_bounded(file, flash_size).map_err(|e| read_error(&e))? {
-        Ok(bytes) => bytes,
-        Err(at_least) => {
-            return Err(FlashFileError::Grew {
-                path: shown,
-                at_least,
-                size: flash_size,
-            });
-        }
-    };
+        },
+        ReadFailure::Grew(at_least) => FlashFileError::Grew {
+            path: shown.clone(),
+            at_least,
+            size: flash_size,
+        },
+    })?;
     if bytes.is_empty() {
         return Err(FlashFileError::Empty { path: shown });
     }
````


````diff
--- a/crates/emu-cli/src/flash_state.rs
+++ b/crates/emu-cli/src/flash_state.rs
@@ -119,17 +119,25 @@
         found: String,
     },
     #[error(
-        "both --flash and the flash file {path}, which exists, were given: the run cannot start from two flashes; drop --flash to go on from the flash file, or add --reset-flash to start again from the image and replace the file"
+        "both {input} and the flash file {path}, which exists, were given: the run cannot start from two flashes; drop {input} to go on from the flash file, or add --reset-flash to start again from {} and replace the file",
+        input.what_it_gives()
     )]
-    BothGiven { path: String },
+    BothGiven { path: String, input: Input },
     #[error(
-        "--flash and --flash-file name the same file, {image} and {path}: the image of --flash is only read, and the flash file is replaced when the run ends; drop --flash to go on from the file, or give --flash-file another path to keep the image as it is"
+        "{input} and --flash-file name the same file, {image} and {path}: {} of {input} is only read, and the flash file is replaced when the run ends; {}",
+        input.noun(), input.same_file_remedy()
     )]
-    SameFile { image: String, path: String },
+    SameFile {
+        input: Input,
+        image: String,
+        path: String,
+    },
     #[error(
-        "--flash names {image}, which is the {role} of the flash file {path}: {why}, and the image of --flash is only read; copy the image to another name, then give that copy to --flash"
+        "{input} names {image}, which is the {role} of the flash file {path}: {why}, and {noun} of {input} is only read; copy {noun} to another name, then give that copy to {input}",
+        noun = input.noun()
     )]
     ImageIsTheRunsOwn {
+        input: Input,
         image: String,
         role: &'static str,
         path: String,
@@ -151,6 +159,64 @@
     sanitize(&path.display().to_string())
 }
 
+/// A file of `inkemu run` that the run only reads, named by its option in the refusals.
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub enum Input {
+    /// `--flash`: a whole flash image.
+    Flash,
+    /// `--firmware`: an application, which a flash is composed around.
+    Firmware,
+    /// `--bootloader`: the bootloader of that flash.
+    Bootloader,
+}
+
+impl Input {
+    /// What the refusals call the file of this input.
+    const fn noun(self) -> &'static str {
+        match self {
+            Input::Flash => "the image",
+            Input::Firmware | Input::Bootloader => "the file",
+        }
+    }
+
+    /// What a run starts from when this input is the seed and the flash file is reset.
+    const fn what_it_gives(self) -> &'static str {
+        match self {
+            Input::Flash => "the image",
+            Input::Firmware | Input::Bootloader => "the flash composed from the firmware",
+        }
+    }
+
+    const fn same_file_remedy(self) -> &'static str {
+        match self {
+            Input::Flash => {
+                "drop --flash to go on from the file, or give --flash-file another path to keep the image as it is"
+            }
+            Input::Firmware | Input::Bootloader => {
+                "give --flash-file another path to keep the file as it is"
+            }
+        }
+    }
+}
+
+impl std::fmt::Display for Input {
+    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
+        f.write_str(match self {
+            Input::Flash => "--flash",
+            Input::Firmware => "--firmware",
+            Input::Bootloader => "--bootloader",
+        })
+    }
+}
+
+/// The content a run starts from when it does not go on from its flash file, and the option
+/// that gave it.
+#[derive(Clone, Debug, PartialEq, Eq)]
+pub struct Seed {
+    pub input: Input,
+    pub content: Vec<u8>,
+}
+
 /// What to do about a file that cannot be created beside the flash file. The lock file and the
 /// temporary file are both named after the flash file with five bytes more: a name too long
 /// for one is too long for the other.
@@ -194,11 +260,13 @@
 /// [`FlashStateError::ImageIsTheRunsOwn`] when it is the temporary file or the lock file of the
 /// flash file, by whatever route the two paths lead to one file.
 pub fn refuse_the_image_as_flash_file(
+    input: Input,
     image: &Path,
     flash_file: &Path,
 ) -> Result<(), FlashStateError> {
     if same_file(image, flash_file) {
         return Err(FlashStateError::SameFile {
+            input,
             image: shown(image),
             path: shown(flash_file),
         });
@@ -220,6 +288,7 @@
             && same_file(image, &path)
         {
             return Err(FlashStateError::ImageIsTheRunsOwn {
+                input,
                 image: shown(image),
                 role,
                 path: shown(flash_file),
@@ -755,7 +824,7 @@
     pub fn begin(
         flash_file: Option<&Path>,
         reset: bool,
-        seed: Option<Vec<u8>>,
+        seed: Option<Seed>,
         size: u32,
     ) -> Result<(RunFlash, Option<Vec<u8>>), FlashStateError> {
         let state = flash_file
@@ -765,9 +834,12 @@
         // size is not a flash of this device, and `--reset-flash` must not destroy it.
         let from_file = match &state {
             Some(state) if state.exists()? => {
-                if !reset && seed.is_some() {
+                if let Some(seed) = &seed
+                    && !reset
+                {
                     return Err(FlashStateError::BothGiven {
                         path: state.shown.clone(),
+                        input: seed.input,
                     });
                 }
                 !reset
@@ -776,7 +848,7 @@
         };
         let content = match &state {
             Some(state) if from_file => Some(state.load()?),
-            _ => seed,
+            _ => seed.map(|seed| seed.content),
         };
         let start_sha256 = content_sha256(content.as_deref().unwrap_or_default(), size);
         Ok((
````


````diff
--- a/crates/emu-cli/src/main.rs
+++ b/crates/emu-cli/src/main.rs
@@ -3,8 +3,9 @@
 use clap::{Parser, Subcommand};
 use cpu_rv32::{HartConfig, MisalignedPolicy};
 use emu_cli::exit::{EXIT_ERROR, EXIT_SECOND_INTERRUPT, EXIT_TEST_FAILURE, RUN_EXIT_CODES_HELP};
+use emu_cli::firmware_file;
 use emu_cli::flash_file;
-use emu_cli::flash_state::{self, RunFlash};
+use emu_cli::flash_state::{self, Input, RunFlash, Seed};
 use emu_cli::rom_store::{
     self, AnnouncingFetch, ESP_ROM_ELFS, ESP32C3_ROMS, HttpFetch, RomEntry, RomSource,
 };
@@ -62,6 +63,19 @@
         /// writes to the flash is kept by --flash-file, or lost when the run ends.
         #[arg(long)]
         flash: Option<PathBuf>,
+        /// Application image to boot (the .bin of an ESP-IDF build or of a release, its first
+        /// byte 0xE9): the flash is composed from it, as a device freshly flashed holds it. The
+        /// bootloader inkemu ships for the device goes at offset 0, the partition table of the
+        /// device profile at 0x8000, the application in the slot the bootloader starts (the
+        /// factory application, else the first OTA slot), and every other byte is erased, the
+        /// OTA data partition included.
+        /// The file is only read. A whole flash image goes to --flash instead.
+        #[arg(long, value_name = "FILE", conflicts_with = "flash")]
+        firmware: Option<PathBuf>,
+        /// Bootloader to compose the flash with instead of the one inkemu ships for the device:
+        /// the bootloader.bin of a build. It is checked against the device and written as it is.
+        #[arg(long, value_name = "FILE", requires = "firmware")]
+        bootloader: Option<PathBuf>,
         /// Keep the flash of the device in this file from one run to the next: a raw image of
         /// exactly the size of the flash. A run starts from the file when it exists (--flash is
         /// then refused unless --reset-flash is given) and writes it when it ends: at a limit,
@@ -78,7 +92,8 @@
         #[arg(long, value_name = "FILE")]
         flash_file: Option<PathBuf>,
         /// Start from the --flash image, or from an erased flash, although the flash file
-        /// exists, and replace the file when the run ends.
+        /// exists, and replace the file when the run ends. With --firmware, start from the flash
+        /// composed from it.
         #[arg(long, requires = "flash_file")]
         reset_flash: bool,
         /// Stop when the CPU has executed this many cycles of its clock. The limit counts executed
@@ -179,6 +194,8 @@
         Command::Run {
             device,
             flash,
+            firmware,
+            bootloader,
             flash_file,
             reset_flash,
             max_cycles,
@@ -191,6 +208,8 @@
             &device,
             &FlashOptions {
                 image: flash.as_deref(),
+                firmware: firmware.as_deref(),
+                bootloader: bootloader.as_deref(),
                 file: flash_file.as_deref(),
                 reset: reset_flash,
             },
@@ -265,6 +284,10 @@
 struct FlashOptions<'a> {
     /// `--flash`: the image a run starts from, never written.
     image: Option<&'a Path>,
+    /// `--firmware`: the application a flash is composed around, never written.
+    firmware: Option<&'a Path>,
+    /// `--bootloader`: the bootloader of that flash, never written.
+    bootloader: Option<&'a Path>,
     /// `--flash-file`: where the flash is kept from one run to the next.
     file: Option<&'a Path>,
     /// `--reset-flash`: start again from the image and replace the file.
@@ -292,32 +315,27 @@
         Err(e) => return fail(&e),
     };
     let flash_size = profile.flash.size();
-    // The flash image and the flash file are checked first: refusing them needs no ROM, and no
-    // download should start for a run that cannot happen.
-    if let (Some(image), Some(file)) = (flash.image, flash.file)
-        && let Err(e) = flash_state::refuse_the_image_as_flash_file(image, file)
-    {
-        return fail(&e);
-    }
-    let image = match flash.image {
-        Some(path) => match flash_file::load(path, profile.soc.chip_id(), flash_size) {
-            Ok(bytes) => Some(bytes),
-            Err(e) => return fail(&e),
-        },
-        None => None,
-    };
-    // What the flash holds when nothing is kept from an earlier run.
-    let fresh_note = match (flash.image, &image) {
-        (Some(path), Some(bytes)) => format!(
-            "{} ({} bytes in {} MiB)",
-            file_name(path),
-            bytes.len(),
-            flash_size >> 20
-        ),
-        _ => format!("erased ({} MiB)", flash_size >> 20),
+    // The files a run only reads and the flash file are checked first: refusing them needs no
+    // ROM, and no download should start for a run that cannot happen.
+    if let Some(file) = flash.file {
+        for (input, path) in [
+            (Input::Flash, flash.image),
+            (Input::Firmware, flash.firmware),
+            (Input::Bootloader, flash.bootloader),
+        ] {
+            if let Some(path) = path
+                && let Err(e) = flash_state::refuse_the_image_as_flash_file(input, path, file)
+            {
+                return fail(&e);
+            }
+        }
+    }
+    let (seed, fresh_note) = match starting_flash(&profile, flash) {
+        Ok(started) => started,
+        Err(code) => return code,
     };
     // From here on the flash file is locked, until the process ends.
-    let (run_flash, content) = match RunFlash::begin(flash.file, flash.reset, image, flash_size) {
+    let (run_flash, content) = match RunFlash::begin(flash.file, flash.reset, seed, flash_size) {
         Ok(begun) => begun,
         Err(e) => return fail(&e),
     };
@@ -398,6 +416,59 @@
     ExitCode::from(code)
 }
 
+/// What the flash holds when nothing is kept from an earlier run (`None`: erased), and how the
+/// header line of the run names it. A refusal is printed, and its exit code given back.
+fn starting_flash(
+    profile: &DeviceProfile,
+    flash: &FlashOptions<'_>,
+) -> Result<(Option<Seed>, String), ExitCode> {
+    let flash_size = profile.flash.size();
+    Ok(match (flash.image, flash.firmware) {
+        (Some(path), _) => match flash_file::load(path, profile.soc.chip_id(), flash_size) {
+            Ok(bytes) => {
+                let note = format!(
+                    "{} ({} bytes in {} MiB)",
+                    file_name(path),
+                    bytes.len(),
+                    flash_size >> 20
+                );
+                let seed = Seed {
+                    input: Input::Flash,
+                    content: bytes,
+                };
+                (Some(seed), note)
+            }
+            Err(e) => return Err(fail(&e)),
+        },
+        (None, Some(path)) => match firmware_file::compose(profile, path, flash.bootloader) {
+            Ok(composed) => {
+                for warning in &composed.warnings {
+                    write_line(io::stderr(), &format_args!("warning: {warning}"));
+                }
+                let app = &composed.application;
+                let note = format!(
+                    "{} ({} {}, ESP-IDF {}) in {} at {:#x}, behind {}, under the partition table of the profile ({} MiB)",
+                    file_name(path),
+                    sanitize(&app.project_name),
+                    sanitize(&app.version),
+                    sanitize(&app.idf_ver),
+                    composed.slot.name,
+                    composed.slot.offset,
+                    sanitize(&composed.bootloader.to_string()),
+                    flash_size >> 20
+                );
+                let seed = Seed {
+                    input: Input::Firmware,
+                    content: composed.bytes,
+                };
+                (Some(seed), note)
+            }
+            Err(e) => return Err(fail(&e)),
+        },
+        (None, None) => (None, format!("erased ({} MiB)", flash_size >> 20)),
+    })
+}
+
 /// Whether whoever was reading standard output went away (`inkemu run | head`, once `head` has
 /// what it wanted). A run that prints learns it from the write that fails; this is for the run
 /// whose firmware prints nothing, which would otherwise go on for nobody.
````


Create `docs/adr/0005-bootloader-embarque.md`:

````markdown
# ADR 0005 — Le bootloader de second étage : embarqué, choisi par le profil, remplaçable

- Date : 2026-10-13
- Statut : accepté (plan SP1b-3c)

## Contexte

Un utilisateur a le plus souvent une image d'application seule : une version de CrossPoint, le `firmware.bin` d'une construction PlatformIO ou ESP-IDF, le firmware d'origine d'un X3 ou d'un X4. La spec l'accepte (section 5.1) : la flash est alors assemblée avec « le bootloader embarqué » et la table de partitions du profil, et la section 3.4 dit de ce bootloader qu'il est « embarqué, un par puce, construit de façon reproductible ». Restait à dire lequel, d'où il vient, ce qu'inkemu en porte, et comment un appareil le choisit.

Le propriétaire du projet a tranché le 12 octobre 2026 : le bootloader est livré avec inkemu, dans le binaire ; il peut être remplacé ; il est choisi automatiquement d'après le profil de l'appareil, de sorte qu'un modèle ajouté plus tard reçoive le sien sans changer de code. Tout ce qui s'y rapporte doit aussi pouvoir se régler depuis une interface graphique (SP2, puis la version web).

Les faits sont ceux du rapport `docs/research/shipped-bootloader-and-firmware-composition.md` (SB ci-dessous) et de `docs/research/flash-writes-and-crosspoint-boot.md` (FW).

## Décision

1. **Le bootloader livré est l'artefact de référence.** C'est le bootloader de second étage d'ESP-IDF v5.5.2 que construit `tools/reference-firmware/build.sh` : 21 040 octets, SHA-256 `601b48456b0a9d5c9f1beac166135e06686d3c5ed0309dd16840ab39887afaa3`, sources ESP-IDF non modifiées, configuration `tools/reference-firmware/sdkconfig.defaults` seule (journal au niveau INFO, sans retour arrière d'application, révision de puce minimale v0.3). Il est versionné dans `third_party/esp-idf-bootloader/esp32c3/bootloader.bin` et inclus dans l'exécutable (`firmware::SHIPPED`). C'est le seul binaire tiers du dépôt. Le job `reference-boot` de la CI vérifie qu'il est octet pour octet celui que produit la construction de référence.
2. **Le profil le choisit par ce qu'il énonce déjà.** Le registre `firmware::SHIPPED` associe à chaque bootloader sa famille de puce et sa famille de mode de flash (DIO pour tous les fichiers Xteink connus, SB §B). Un profil le désigne par `soc.family` ; il n'a pas de clé de plus. Ajouter un modèle d'une puce déjà connue, c'est écrire un profil ; ajouter une famille de puce, c'est une entrée du registre et son répertoire dans `third_party/esp-idf-bootloader/`. Un profil dont la famille n'a pas de bootloader livré est refusé au chargement, en le nommant. Un test vérifie que chaque profil livré en a un.
3. **Un binaire par famille, pas par taille de flash.** Le bootloader lit la taille de la flash dans son propre en-tête et la transmet à l'application, qui s'arrête si la puce qu'elle trouve est plus petite (SB §B). Pour un appareil dont la flash ne fait pas 16 Mio, l'assemblage écrit la taille de l'appareil dans l'en-tête (quartet haut de l'octet 3) et recalcule l'empreinte ajoutée, comme `esptool.py` le fait de toute image écrite à l'adresse 0 ; le résultat pour 4 Mio est la construction mesurée pour 4 Mio, octet pour octet, et un test l'épingle. Le fichier versionné n'est jamais modifié. Les profils actuels ont tous 16 Mio : le bootloader y est écrit tel quel.
4. **Il est remplaçable.** `inkemu run --bootloader <fichier>` (et, pour une interface, `DeviceProfile::compose_flash` avec le bootloader en octets) assemble la flash avec un autre bootloader, celui de la construction de l'utilisateur par exemple. Ce bootloader est contrôlé avant le démarrage et n'est jamais réécrit : ce que la ROM refuserait (magie, nombre et adresses des segments, somme de contrôle), une puce d'une autre famille, une révision minimale supérieure à celle du profil, une taille de flash d'en-tête supérieure à celle de l'appareil, une image entière ou une application donnée à sa place. Une empreinte ajoutée fausse et une taille de flash d'en-tête plus petite sont des avertissements : la ROM démarre l'une, l'application vit avec l'autre. Le résumé de l'exécution dit quel bootloader est en place : celui qu'inkemu livre, un bootloader connu par son empreinte, ou ce que dit son descripteur (version d'ESP-IDF, date). inkemu n'embarque ni ne télécharge aucun bootloader tiers, celui d'Arduino-ESP32 compris (FW §N) ; un utilisateur peut en donner un.
5. **Aucune puce ne boucle en silence.** Un profil dont la révision est inférieure au minimum du bootloader livré est refusé avant le démarrage : ce bootloader réinitialiserait la puce sans fin (SB §B). Le message propose `--bootloader` avec une construction qui l'accepte, ou un profil de révision suffisante.
6. **Les obligations de licence sont tenues dans le dépôt et dans le binaire.** Le bootloader est du code ESP-IDF sous Apache-2.0, avec des macros de deux en-têtes sous BSD-3-Clause (`riscv/csr.h`, `riscv/encoding.h`) ; rien de GPL ni de LGPL n'y est lié (SB §A). Comme pour les SVD (ADR 0001), `third_party/esp-idf-bootloader/` contient `LICENSE` (le texte d'ESP-IDF v5.5.2), `NOTICE` (l'attribution et les deux avis BSD-3-Clause) et `PROVENANCE.md` (image Docker, révision d'ESP-IDF, chaîne de compilation, configuration, commande, empreinte, « sources non modifiées, configuration seule », réécriture d'en-tête du point 3) ; le `NOTICE` racine a un paragraphe. Le binaire distribué embarque ces octets : `inkemu licenses` imprime le `NOTICE` racine puis l'avis et la licence de chaque partie tierce, à partir de `boards::notices`, que la page « À propos » d'une interface appellera.

## Pourquoi cette configuration

- **Elle est déjà épinglée et vérifiée** : empreinte dans `SHA256SUMS.inputs`, journal de démarrage de référence sous QEMU, tests de la machine et de la commande, empreinte de trace de la vie de `hello_world`.
- **Ses lignes INFO sont la seule vue de l'utilisateur sur ce que décide le bootloader** : la table de partitions lue, la partition choisie, l'écriture d'`otadata` au premier démarrage. Les bootloaders au niveau ERROR (celui d'Arduino-ESP32, une construction ERROR avec retour arrière) n'impriment rien entre la ROM et l'application.
- **Un second bootloader « silencieux »** (niveau ERROR, retour arrière, validation sautée au réveil, plus proche de celui qu'embarque l'écosystème Arduino) est reproductible lui aussi (SB §D) ; il doublerait les empreintes épinglées, l'oracle (un journal QEMU de plus) et le temps de construction de la CI (7 à 10 s par bootloader dans un conteneur chaud). C'est un ajout possible, pas un remplacement : une seconde entrée du registre, qu'un profil ou une interface choisirait.
- Aucune configuration ne reproduit le bootloader d'origine d'un X3 ou d'un X4, qui n'est connu par aucun dépôt (FW §O.1).

## ESP32-S3

La même recette construit le bootloader de l'ESP32-S3 sans autre changement que la cible : 20 992 octets, SHA-256 `53765fcd83f60ce93f280f857999bf2c05d6ce52e6063164ef64301ecd8a023e`, reproductible, placé à l'adresse 0 comme celui du C3, révision minimale v0.0 (SB §E). Ses en-têtes ajoutent des avis de type MIT de Tensilica et Cadence, à joindre au `NOTICE` du répertoire. Il entre dans le dépôt avec le modèle de l'ESP32-S3 (sous-projet 7), en même temps que sa ROM dans le manifeste.

## Conséquences

- La spec n'est pas modifiée : sa section 3.4 dit déjà « embarqué, un par puce ».
- Un appareil émulé démarre derrière un bootloader qui n'est pas celui de l'appareil réel. Les différences connues sont de comportement, pas de refus : niveau de journal, retour arrière d'application, validation de l'image au réveil d'un sommeil profond (FW §I). Un utilisateur qui a le bootloader de son appareil (les 0x8000 premiers octets d'un dump) le donne à `--bootloader`, ou donne le dump entier à `--flash`.
- Un appareil dont la flash est pilotée en QIO, ou en octal, demandera une autre construction et une clé du profil qui nomme le mode de flash : une construction QIO est un autre code, dont l'en-tête dit pourtant DIO (SB §B).
- Installer une nouvelle application sur une flash qui existe déjà, en gardant NVS (une mise à jour comparable à une OTA), n'est pas `--firmware` : `--firmware` est une image de départ comme `--flash` (ADR 0004), refusée à côté d'un fichier de flash qui existe sauf avec `--reset-flash`. Choisir la partition et l'état d'`otadata` d'une telle mise à jour revient à l'interface (SP2).
- Les licences des bibliothèques Rust liées dans l'exécutable ne sont pas encore listées par `inkemu licenses` : elles reviennent à la distribution (sous-projet 9).
````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p emu-cli --lib > /tmp/t9.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t9.log
cargo test -p emu-cli --test run_cli > /tmp/t9b.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t9b.log
cargo test --workspace > /tmp/t9-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t9-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `160 passed`; `exit=0`, `46 passed; 0 failed; 28 ignored`; then `exit=0` and `1064 60` (tests passed and ignored in the workspace).

- [ ] **Step 5: Ignored tests, with the counts of the CI**

With `INKEMU_ROM_ELF_DIR`, `INKEMU_REF_FLASH`, `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH` set (absolute paths):

```bash
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/ign1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 24 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware > /tmp/ign2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 13 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/ign3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 4 --release -p emu-cli --test run_cli reference_firmware -- --ignored > /tmp/ign4.log 2>&1; echo "exit=$?"
```

Expected: four `exit=0`, with `18`, `24`, `13` and `4` passed. The trace hash `10174cc2726c3d77` of the `--flash` life is unchanged (`x4_runs_the_reference_firmware_along_its_pinned_trace`).

- [ ] **Step 6: Mutations**

Each row: apply the change, run `cargo test -p emu-cli --test run_cli (the last row: cargo test -p emu-cli --lib firmware_file)`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `main.rs`: delete the `(Input::Firmware, flash.firmware),` and `(Input::Bootloader, flash.bootloader),` rows of the loop | `a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply` |
| 2 | `flash_state.rs`: `input: seed.input,` → `input: Input::Flash,` | `a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply` |
| 3 | `#[arg(long, value_name = "FILE", conflicts_with = "flash")]` → `#[arg(long, value_name = "FILE")]` | `a_firmware_with_an_image_or_a_bootloader_without_a_firmware_is_a_usage_error`, `a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply` |
| 4 | `firmware_file::compose(profile, path, flash.bootloader)` → `firmware_file::compose(profile, path, None)` | `every_input_that_cannot_be_booted_from_an_application_is_refused_before_boot`, `a_firmware_is_a_seed_like_an_image_and_the_flash_file_rules_apply` |
| 5 | `firmware_file.rs`: `(ComposeError::Bootloader(_), Some(path)) => shown(path),` → `(ComposeError::Bootloader(_), Some(_)) => shown(firmware),` | `a_refusal_names_the_file_it_is_about` |
| 6 | `flash_file.rs`: `if metadata.len() > u64::from(limit) {` → `if false {` | `a_file_that_is_missing_too_long_or_no_file_is_refused_before_it_is_read` (it sees `Grew`, not `TooLong`), `image_longer_than_the_flash_is_refused_with_both_sizes` |

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add crates/emu-cli/src/firmware_file.rs docs/adr/0005-bootloader-embarque.md
/usr/bin/git commit -m "feat(cli): inkemu run --firmware boots an application image behind the bootloader the device ships with, or --bootloader" -- crates/emu-cli/src/firmware_file.rs crates/emu-cli/src/flash_file.rs crates/emu-cli/src/flash_state.rs crates/emu-cli/src/lib.rs crates/emu-cli/src/main.rs crates/emu-cli/tests/run_cli.rs docs/adr/0005-bootloader-embarque.md
```

Expected: three `exit=0`, then the commit.

---

## Task 10: The reference firmware boots and counts from its application images

The milestone on Espressif's own code. The two applications of the reference build are cut out of the pinned merged images at `0x10000` and checked against `SHA256SUMS.inputs` and `SHA256SUMS.inputs-nvs` (C25): no Docker run, no new file. Four ignored tests, selected by `reference_firmware` in the CI: (1) `--firmware hello_world.bin` in strict mode, 44 million cycles: the 26 bootloader lines of the QEMU capture of the same flash (`golden_hello_ota_boot.log`, TO §2, with the documented eFuse block revision and SPI mode), the life, the restart, the second boot, `otadata` written once; (2) the trace hash of 40,000,001 cycles, with its device time and the composed flash's SHA-256, the reference build merged under the X4 table; (3) the NVS example from its application and a flash file: the first run composes `flash_nvs_ota.bin` byte for byte, the second goes on counting; (4) the shipped bootloader is the bytes the reference build merges at offset 0 (C1, C26). These tests pass on the code of Task 9: they are the proof of the milestone, and their mutations below show what they hold.

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `crates/emu-cli/tests/run_cli.rs`
- Modify: `tools/reference-firmware/README.md`

**Interfaces:**
- Consumes: `inkemu run --firmware`, `firmware::FirmwareImage::parse(..).len`, `firmware::SHIPPED`, the helpers of `run_cli.rs` (`pinned_image`, `reference_image`, `trace_hash_line`, `device_micros`, `Counted`, `sha256_of`, `FLASH_AT_START`/`FLASH_AT_END`).
- Produces: four ignored tests; the reference-boot count of `run_cli` 4 → 8.

**Bounds on untrusted input.** The merged images are pinned by SHA-256 before anything is cut out of them.

- [ ] **Step 1: Write the tests**

````diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -2911,3 +2911,295 @@
         assert!(out.contains(expected), "`{expected}` is missing:\n{out}");
     }
 }
+
+/// `hello_world.bin` and `nvs_rw_value_cxx.bin`, the application images the reference build
+/// merges at 0x10000 (`tools/reference-firmware/SHA256SUMS.inputs`, `SHA256SUMS.inputs-nvs`).
+const HELLO_APPLICATION_SHA256: &str =
+    "4b82d73e37d10969d660eba95984cbe7f053212d39306cc54bfce6c366d0f04b";
+const NVS_APPLICATION_SHA256: &str =
+    "44523a2fd8f7dc0a3eaad17a8991c647ab663ad993dc0db2e31235806b516ff2";
+/// The flash composed for the X4 from each application: the reference images merged under the
+/// stock table of the X4, `flash_hello_ota.bin` (docs/research/sp1b-test-oracle.md, section 2) and
+/// `flash_nvs_ota.bin` (`SHA256SUMS.output`).
+const HELLO_COMPOSED_SHA256: &str =
+    "9ddacb60b8b7338731b52edee1ecae47c7f45470d8db6fa8d1fbc5a20011bc89";
+const NVS_COMPOSED_SHA256: &str =
+    "87832c6d5d15672fb3892bfad281688fc7486b360a25b495bdb6a27d04f28fec";
+
+/// The application image of the pinned merged image `variable` names, written to a file of its
+/// own: the bytes the reference build merged at 0x10000, checked against `application_sha256`.
+fn reference_application(
+    variable: &str,
+    file: &str,
+    sha256: &str,
+    application_sha256: &str,
+) -> (tempfile::TempDir, std::path::PathBuf) {
+    use sha2::Digest as _;
+    let merged = std::fs::read(pinned_image(variable, file, sha256)).unwrap();
+    let from = &merged[0x1_0000..];
+    let len = firmware::FirmwareImage::parse(from).unwrap().len;
+    let application = &from[..len];
+    assert_eq!(
+        emu_cli::term::hex(&sha2::Sha256::digest(application)),
+        application_sha256,
+        "the application merged at 0x10000 of {file}"
+    );
+    let dir = tempfile::tempdir().unwrap();
+    let path = dir.path().join(file.replace("flash_", "app_"));
+    std::fs::write(&path, application).unwrap();
+    (dir, path)
+}
+
+/// A console with the timestamps of ESP-IDF's log lines masked: `I (123) boot: …` → `I (#) boot: …`.
+fn masked(console: &str) -> String {
+    console
+        .lines()
+        .map(|line| match line.split_once(") ") {
+            Some((head, rest))
+                if head.len() > 3
+                    && head[..3].ends_with(" (")
+                    && head[3..].bytes().all(|b| b.is_ascii_digit()) =>
+            {
+                format!("{} (#) {rest}", &head[..1])
+            }
+            _ => line.to_owned(),
+        })
+        .collect::<Vec<_>>()
+        .join("\n")
+}
+
+/// The first boot of the application behind the stock table of the X4: the lines of the QEMU
+/// capture of the same flash (`golden_hello_ota_boot.log`, docs/research/sp1b-test-oracle.md,
+/// section 2), with the two documented differences of inkemu's chip: eFuse block revision v1.2
+/// and SPI mode DIO.
+const OTA_FIRST_BOOT_LINES: [&str; 26] = [
+    "I (#) boot: ESP-IDF v5.5.2 2nd stage bootloader",
+    "I (#) boot: chip revision: v0.3",
+    "I (#) boot: efuse block revision: v1.2",
+    "I (#) boot.esp32c3: SPI Speed      : 80MHz",
+    "I (#) boot.esp32c3: SPI Mode       : DIO",
+    "I (#) boot.esp32c3: SPI Flash Size : 16MB",
+    "I (#) boot: Enabling RNG early entropy source...",
+    "I (#) boot: Partition Table:",
+    "I (#) boot: ## Label            Usage          Type ST Offset   Length",
+    "I (#) boot:  0 nvs              WiFi data        01 02 00009000 00005000",
+    "I (#) boot:  1 otadata          OTA data         01 00 0000e000 00002000",
+    "I (#) boot:  2 app0             OTA app          00 10 00010000 00640000",
+    "I (#) boot:  3 app1             OTA app          00 11 00650000 00640000",
+    "I (#) boot:  4 spiffs           Unknown data     01 82 00c90000 00360000",
+    "I (#) boot:  5 coredump         Unknown data     01 03 00ff0000 00010000",
+    "I (#) boot: End of partition table",
+    "I (#) boot: No factory image, trying OTA 0",
+    "I (#) esp_image: segment 0: paddr=00010020 vaddr=3c010020 size=0656ch ( 25964) map",
+    "I (#) esp_image: segment 1: paddr=00016594 vaddr=3fc8b600 size=011d8h (  4568) load",
+    "I (#) esp_image: segment 2: paddr=00017774 vaddr=40380000 size=088a4h ( 34980) load",
+    "I (#) esp_image: segment 3: paddr=00020020 vaddr=42000020 size=0e6fch ( 59132) map",
+    "I (#) esp_image: segment 4: paddr=0002e724 vaddr=403888a4 size=02c70h ( 11376) load",
+    "I (#) esp_image: segment 5: paddr=0003139c vaddr=50000000 size=00020h (    32) load",
+    "I (#) boot: Loaded app from partition at offset 0x10000",
+    "I (#) boot: Set actual ota_seq=1 in otadata[0]",
+    "I (#) boot: Disabling RNG early entropy source...",
+];
+
+/// The milestone of an application image: `--firmware hello_world.bin`, the application alone,
+/// boots behind the bootloader inkemu ships and the stock table of the X4, in strict mode,
+/// through its whole life, its restart and the second boot; the bootloader writes `otadata` on
+/// the first boot and on that one only.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
+fn x4_boots_the_reference_firmware_from_its_application_image_through_a_second_boot() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let (_dir, application) = reference_application(
+        "INKEMU_REF_FLASH",
+        "flash_hello.bin",
+        HELLO_IMAGE_SHA256,
+        HELLO_APPLICATION_SHA256,
+    );
+    // One life is 40 million cycles here; 44 million reach the application of the second boot.
+    let output = inkemu()
+        .args([
+            "run",
+            "--device",
+            "x4",
+            "--strict-mmio",
+            "--max-cycles",
+            "44000000",
+        ])
+        .args(["--rom-dir", &rom_dir, "--firmware"])
+        .arg(&application)
+        .timeout(HANG_LIMIT)
+        .assert()
+        .code(0)
+        .get_output()
+        .clone();
+    let console = masked(&String::from_utf8_lossy(&output.stdout).replace('\r', ""));
+    let first_boot = OTA_FIRST_BOOT_LINES.join("\n");
+    assert!(console.contains(&first_boot), "{console}");
+    let mut from = 0;
+    for expected in [
+        "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
+        "entry 0x403cbf1a\n",
+        first_boot.as_str(),
+        "I (#) cpu_start: Unicore app\n",
+        "Hello world!\n",
+        "Restarting in 0 seconds...\n",
+        "Restarting now.\n",
+        "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
+        "I (#) boot: End of partition table\nI (#) esp_image: segment 0:",
+        "I (#) boot: Loaded app from partition at offset 0x10000\nI (#) boot: Disabling RNG early entropy source...\n",
+        "I (#) cpu_start: Unicore app\n",
+    ] {
+        let at = console[from..]
+            .find(expected)
+            .unwrap_or_else(|| panic!("`{expected}` is missing, or out of order:\n{console}"));
+        from += at + expected.len();
+    }
+    // The second boot finds otadata written: it neither looks for a factory image nor writes.
+    assert_eq!(
+        console
+            .matches("Set actual ota_seq=1 in otadata[0]")
+            .count(),
+        1
+    );
+    assert_eq!(console.matches("No factory image, trying OTA 0").count(), 1);
+    assert_eq!(console.matches("2nd stage bootloader").count(), 2);
+    let log = String::from_utf8_lossy(&output.stderr).into_owned();
+    assert!(
+        log.contains(
+            "flash: app_hello.bin (hello_world 1, ESP-IDF v5.5.2) in app0 at 0x10000, behind the ESP-IDF v5.5.2 bootloader inkemu ships (esp-idf-v5.5.2-esp32c3-dio), under the partition table of the profile (16 MiB)\n"
+        ),
+        "{log}"
+    );
+    assert!(
+        !log.contains("warning:") && !log.contains("stopped by --strict-mmio"),
+        "nothing was reported in a whole life cycle: {log}"
+    );
+}
+
+/// The trace hash of the application image's life, pinned like the one of the merged image
+/// (`x4_runs_the_reference_firmware_along_its_pinned_trace`), with its device time and the flash
+/// it starts from. The two differ from those of the merged image: the table is the X4's, so the
+/// bootloader takes the OTA path and writes `otadata`.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_REF_FLASH"]
+fn x4_runs_the_reference_firmware_from_its_application_image_along_its_pinned_trace() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let (_dir, application) = reference_application(
+        "INKEMU_REF_FLASH",
+        "flash_hello.bin",
+        HELLO_IMAGE_SHA256,
+        HELLO_APPLICATION_SHA256,
+    );
+    let output = inkemu()
+        .args([
+            "run",
+            "--device",
+            "x4",
+            "--strict-mmio",
+            "--max-cycles",
+            "40000001",
+        ])
+        .args(["--trace-hash", "--rom-dir", &rom_dir, "--firmware"])
+        .arg(&application)
+        .timeout(HANG_LIMIT)
+        .assert()
+        .code(0)
+        .get_output()
+        .clone();
+    let log = String::from_utf8_lossy(&output.stderr).into_owned();
+    assert_eq!(
+        trace_hash_line(&log),
+        Some("trace-hash c6b24a1cc226ed4a"),
+        "{log}"
+    );
+    assert_eq!(device_micros(&log), 11_403_054, "{log}");
+    assert!(
+        log.contains("after 40000001 cycles (26637404 instructions)"),
+        "{log}"
+    );
+    assert!(
+        log.contains(&format!("{FLASH_AT_START}{HELLO_COMPOSED_SHA256}\n")),
+        "the composed flash is the merged image under the X4 table: {log}"
+    );
+}
+
+/// The NVS example from its application image, kept in a flash file: the first run composes the
+/// flash (it is `flash_nvs_ota.bin`, byte for byte), the second goes on from the file and counts.
+#[test]
+#[ignore = "needs INKEMU_ROM_ELF_DIR and INKEMU_NVS_FLASH"]
+fn the_nvs_reference_firmware_counts_on_from_its_application_image_across_two_runs() {
+    let rom_dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
+    let (dir, application) = reference_application(
+        "INKEMU_NVS_FLASH",
+        "flash_nvs.bin",
+        NVS_IMAGE_SHA256,
+        NVS_APPLICATION_SHA256,
+    );
+    let file = dir.path().join("x4.bin");
+    let run = |more: &[&std::ffi::OsStr]| {
+        let output = inkemu()
+            .args(["run", "--device", "x4", "--strict-mmio", "--max-time", "5"])
+            .args(["--rom-dir", &rom_dir])
+            .args(more)
+            .arg("--flash-file")
+            .arg(&file)
+            .timeout(HANG_LIMIT)
+            .output()
+            .unwrap();
+        Counted {
+            code: output.status.code(),
+            console: String::from_utf8_lossy(&output.stdout).replace('\r', ""),
+            log: String::from_utf8_lossy(&output.stderr).into_owned(),
+        }
+    };
+    let first = run(&[std::ffi::OsStr::new("--firmware"), application.as_os_str()]);
+    assert_eq!(first.code, Some(0), "{}", first.log);
+    assert_eq!(first.sha256(FLASH_AT_START), NVS_COMPOSED_SHA256);
+    assert!(
+        first
+            .console
+            .contains("boot: Set actual ota_seq=1 in otadata[0]\n")
+            && first.console.contains(
+                "Reading restart counter from NVS ... The value is not initialized yet!\n"
+            ),
+        "{}",
+        first.console
+    );
+    assert_eq!(first.sha256(FLASH_AT_END), sha256_of(&file));
+    let second = run(&[]);
+    assert_eq!(second.code, Some(0), "{}", second.log);
+    assert_eq!(second.sha256(FLASH_AT_START), first.sha256(FLASH_AT_END));
+    assert!(
+        second.console.contains("rst:0x1 (POWERON)")
+            && !second.console.contains("Set actual ota_seq")
+            && second
+                .console
+                .contains("Reading restart counter from NVS ... Done\nRestart counter = 1\n"),
+        "{}",
+        second.console
+    );
+    for log in [&first.log, &second.log] {
+        assert!(!log.contains("warning:"), "{log}");
+    }
+    assert_eq!(
+        sha256_of(&application),
+        NVS_APPLICATION_SHA256,
+        "--firmware is only read"
+    );
+}
+
+/// The bootloader inkemu ships is the one the reference build produces: the first bytes of the
+/// pinned merged image, which the build checks against the same SHA-256.
+#[test]
+#[ignore = "needs INKEMU_REF_FLASH"]
+fn the_shipped_bootloader_is_the_one_the_reference_firmware_build_produced() {
+    let merged = std::fs::read(reference_image()).unwrap();
+    let shipped = firmware::SHIPPED[0].bytes;
+    assert_eq!(&merged[..shipped.len()], shipped);
+    assert!(
+        merged[shipped.len()..0x8000]
+            .iter()
+            .all(|&byte| byte == 0xFF),
+        "nothing follows the bootloader before the table"
+    );
+}
````


- [ ] **Step 2: The CI count and the README of the reference firmware**

````diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -96,7 +96,8 @@
   reference-boot:
     # Boots the reference firmware (stock ESP-IDF bootloader + hello_world) from the real mask ROM
     # and runs it through a whole life: the application, its countdown, the restart it asks for
-    # and the second boot. A second example, nvs_rw_value_cxx, keeps a restart counter in NVS:
+    # and the second boot; then the same application alone, behind the bootloader inkemu ships,
+    # whose bytes a test compares with those of this build. A second example, nvs_rw_value_cxx, keeps a restart counter in NVS:
     # it is built next to the first, once under its own partition table and once under the stock
     # table of the X4. The firmware is built here, inside the pinned ESP-IDF image, and checked
     # against pinned SHA-256 values; it is never stored in this repository. The cache saves the
@@ -146,4 +147,4 @@
         # Exact test counts, as in rom-boot: see tools/ci/run-ignored-tests.sh.
         run: |
           tools/ci/run-ignored-tests.sh 13 --release -p soc-esp32c3 --test reference_boot -- --ignored
-          tools/ci/run-ignored-tests.sh 4 --release -p emu-cli --test run_cli reference_firmware -- --ignored
+          tools/ci/run-ignored-tests.sh 8 --release -p emu-cli --test run_cli reference_firmware -- --ignored
````


````diff
--- a/tools/reference-firmware/README.md
+++ b/tools/reference-firmware/README.md
@@ -32,6 +32,18 @@
     cargo test --release -p soc-esp32c3 --test reference_boot -- --ignored
     cargo test --release -p emu-cli --test run_cli reference_firmware -- --ignored
 
+The tests of `inkemu run --firmware` boot the application images alone: they take them out of the
+merged images, from 0x10000, and check them against `SHA256SUMS.inputs` and `SHA256SUMS.inputs-nvs`
+before they run them. To boot one by hand, cut it out the same way (`hello_world.bin` is the 136,160
+bytes from 65,536 of `flash_hello.bin`, `nvs_rw_value_cxx.bin` the 161,312 bytes from 65,536 of
+`flash_nvs.bin`):
+
+    dd if=target/reference-firmware/flash_hello.bin of=hello_world.bin bs=4096 skip=65536 count=136160 iflag=skip_bytes,count_bytes
+    cargo run --release -p emu-cli -- run --device x4 --firmware hello_world.bin --max-time 12
+
+The bootloader of this build is the one inkemu ships (`third_party/esp-idf-bootloader/`): a test
+compares the first bytes of `flash_hello.bin` with it.
+
 The paths are absolute because each test binary runs in the directory of its own crate. Every test
 checks the SHA-256 of the image it is given before it runs it, and fails by naming the variable or
 the file when the image is missing or is not the pinned one.
````


- [ ] **Step 3: Run the tests**

```bash
cargo test -p emu-cli --test run_cli > /tmp/t10.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t10.log
cargo test --workspace > /tmp/t10-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t10-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `46 passed; 0 failed; 32 ignored` (the four new tests are ignored: the next step runs them); then `exit=0` and `1064 64` (tests passed and ignored in the workspace).

- [ ] **Step 4: Ignored tests, with the counts of the CI**

With `INKEMU_ROM_ELF_DIR`, `INKEMU_REF_FLASH`, `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH` set (absolute paths):

```bash
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/ign1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 24 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware > /tmp/ign2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 13 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/ign3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 8 --release -p emu-cli --test run_cli reference_firmware -- --ignored > /tmp/ign4.log 2>&1; echo "exit=$?"
```

Expected: four `exit=0`, with `18`, `24`, `13` and `8` passed. The trace hash `10174cc2726c3d77` of the `--flash` life is unchanged (`x4_runs_the_reference_firmware_along_its_pinned_trace`).

- [ ] **Step 5: Mutations**

Each row: apply the change, run `cargo test --release -p emu-cli --test run_cli application_image -- --ignored`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `crates/boards/profiles/x4.toml`: the `app0` row's `subtype = "ota_0"` → `"factory"` | the three `…application_image…` tests |
| 2 | `firmware/src/compose.rs`: `let app_at = slot.offset as usize;` → `slot.offset as usize + 0x1_0000` | the three `…application_image…` tests |

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git commit -m "test(cli): the reference firmware boots and counts from its application images, behind the bootloader inkemu ships" -- .github/workflows/ci.yml crates/emu-cli/tests/run_cli.rs tools/reference-firmware/README.md
```

Expected: three `exit=0`, then the commit.

---

## Task 11: Hand-over of SP1b-3c; the README boots an application image

What is left: the refusal of an application given to `--flash` proposes `--firmware` (C29); the comment of `accepted_without_effect.rs` no longer lists SP1b-3c among the plans to come; the README (English, then French) says how to boot an application image, regenerates the help it quotes and lists `inkemu licenses` and ADR 0005; the hand-over note, in French, records the decisions, the new `ASSUMPTION:`, the heirs and what to carry into the spec. The final gates run on the whole branch: coverage, `cargo deny`, every ignored suite with its CI count.

**Files:**
- Modify: `README.md`
- Modify: `crates/emu-cli/tests/run_cli.rs`
- Modify: `crates/firmware/src/error.rs`
- Modify: `crates/soc-esp32c3/tests/accepted_without_effect.rs`
- Create: `docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md`

**Interfaces:**
- Consumes: everything above.
- Produces: `docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md`; the README; the message of `FirmwareError::NoPartitionTable`.

**Bounds on untrusted input.** None new.

- [ ] **Step 1: Write the failing tests**

````diff
--- a/crates/emu-cli/tests/run_cli.rs
+++ b/crates/emu-cli/tests/run_cli.rs
@@ -458,7 +458,11 @@
 fn an_application_image_given_as_flash_is_refused() {
     run_with_flash(&build_image(&ImageSpec::default()))
         .code(2)
-        .stderr(contains("no partition table at offset 0x8000").and(contains("merge_bin")));
+        .stderr(
+            contains("no partition table at offset 0x8000")
+                .and(contains("; use an application with --firmware"))
+                .and(contains("merge_bin")),
+        );
 }
 
 /// `inkemu run --device x4 --flash <path>` with an empty ROM directory: a refusal of the flash file
````


````diff
--- a/crates/soc-esp32c3/tests/accepted_without_effect.rs
+++ b/crates/soc-esp32c3/tests/accepted_without_effect.rs
@@ -37,7 +37,7 @@
 /// The plans a row may hand its register to: the plans still to come that own at least one row
 /// of the table. A plan that is done cannot be an heir: what it did not do is handed on by name
 /// before it ends, and its name is removed from this list by its last task. A plan to come that
-/// owns no row (SP1b-3c and SP1b-3e today) is not in the list: it is added with its first row.
+/// owns no row (SP1b-3e today) is not in the list: it is added with its first row.
 /// The list holds the table only: a comment or a document that names an heir is not read here.
 const PLANS_TO_COME: [&str; 7] = ["SP1b-3d", "SP1b-3f", "SP1b-4", "SP2", "SP5a", "SP5b", "SP8"];
 
````


- [ ] **Step 2: Run them and see them fail**

```bash
cargo test -p emu-cli --test run_cli an_application_image_given_as_flash_is_refused > /tmp/t11-red.log 2>&1; echo "exit=$?"; grep "FAILED" /tmp/t11-red.log
```

Expected: `exit=101`; `test an_application_image_given_as_flash_is_refused ... FAILED` (the message does not name `--firmware`).

- [ ] **Step 3: The message, the comment, the README, the note**

````diff
--- a/README.md
+++ b/README.md
@@ -2,7 +2,7 @@
 
 **A binary-level emulator for embedded e-ink devices.** inkemu runs the real, unmodified firmware image of a device on a virtual replica of its hardware. You can check how a firmware renders and behaves before you flash it.
 
-> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The flash is written as on the device: the bootloader records the OTA slot it chose, NVS keeps what the firmware stores, and `--flash-file` keeps the flash of a device from one run to the next. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Next come a firmware given on its own (sub-project 1b-3c), the serial consoles in both directions (1b-3d), a community firmware (1b-3e), deep sleep (1b-3f) and the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
+> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector. From a full flash image, the ROM loads the stock ESP-IDF second-stage bootloader, which starts the application; the application runs its whole life — FreeRTOS scheduler, tick interrupt, timers, watchdogs — and when it restarts, the chip boots again. The flash is written as on the device: the bootloader records the OTA slot it chose, NVS keeps what the firmware stores, and `--flash-file` keeps the flash of a device from one run to the next. An application `.bin` given on its own boots too: inkemu composes the flash around it with the ESP-IDF bootloader it ships and the partition table of the device. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Next come the serial consoles in both directions (1b-3d), a community firmware (1b-3e), deep sleep (1b-3f) and the lockstep against QEMU (1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).
 
 ## Build & try
 
@@ -45,10 +45,10 @@
 
 ```text
 $ target/release/inkemu run --device x4 --flash app.bin
-error: app.bin: the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)
-```
-
-Without `--flash` the flash is erased and the ROM prints `invalid header: 0xffffffff` in a loop, as a blank chip does. There is no option yet to boot an application `.bin` on its own: inkemu does not compose a flash from one (planned for sub-project 1b-3c).
+error: app.bin: the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use an application with --firmware, or give --flash a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)
+```
+
+Without `--flash` or `--firmware` the flash is erased and the ROM prints `invalid header: 0xffffffff` in a loop, as a blank chip does. An application `.bin` on its own goes to `--firmware` (below).
 
 The reference image (`hello_world` of ESP-IDF v5.5.2, saved as `flash_hello.bin`) runs its whole life: it prints `Hello world!`, counts down eleven seconds, calls `esp_restart()`, and the ROM boots again, announcing `rst:0xc (RTC_SW_CPU_RST)`. The seconds the application spends waiting are not executed: the CPU sleeps in `wfi` and device time jumps to the next timer event, so the eleven seconds of the countdown take about 0.05 s of host time, and the whole run below, 50 million cycles of the CPU, about 1.1 s (measured on one development machine). With `--max-cycles 50000000`, standard error reads:
 
@@ -74,6 +74,24 @@
 ```
 
 The last line names the flash the run started from: the whole chip, erased bytes included, so it is what `sha256sum` says of a 16 MiB image. It is an input of the run, as the ROM and the device are.
+
+### Booting an application image
+
+Most firmware comes as an application image alone: a release of a community firmware, the `firmware.bin` of a PlatformIO build, the `.bin` of an ESP-IDF build. `--firmware` boots it as a freshly flashed device would:
+
+```sh
+target/release/inkemu run --device x4 --firmware hello_world.bin --max-time 12
+```
+
+inkemu composes the flash first: at offset 0 the ESP-IDF v5.5.2 2nd-stage bootloader it ships (built from unmodified sources, see [ADR 0005](docs/adr/0005-bootloader-embarque.md)), at 0x8000 the partition table of the device profile (for the X4, its stock table), the application in the slot the bootloader starts (the factory application, else the first OTA slot), and every other byte erased. The bootloader is chosen by the profile, from its chip; `--bootloader <file>` puts another one in its place, the `bootloader.bin` of your build for instance. Standard error says what the flash is made of:
+
+```text
+inkemu: Xteink X4 (ESP32-C3 v0.3, mask ROM rev3); timing model datasheet-uncalibrated (not calibrated); flash: hello_world.bin (hello_world 1, ESP-IDF v5.5.2) in app0 at 0x10000, behind the ESP-IDF v5.5.2 bootloader inkemu ships (esp-idf-v5.5.2-esp32c3-dio), under the partition table of the profile (16 MiB)
+```
+
+On the first boot the bootloader prints the partition table, `No factory image, trying OTA 0` and `Set actual ota_seq=1 in otadata[0]`, as on a device whose `otadata` is blank. The same files give the same flash: its SHA-256 is printed as `flash at start`. A file that cannot be booted is refused before the run, with the cause and the remedy (exit code 2): a file that is not an ESP-IDF image, an encrypted `.xota`, an image for another chip (an ESP32-S3 image on an X4), an application larger than its slot, a corrupt checksum or digest, a whole flash image (it goes to `--flash`), a bootloader given as the firmware or an application given as the bootloader, an application or a bootloader that requires a later chip revision than the device has. `--firmware` is only read and, like `--flash`, is refused next to a flash file that exists unless `--reset-flash` is given.
+
+`inkemu licenses` prints the notices and licences of what the binary carries from third parties: the register descriptions and the bootloader.
 
 ### Keeping the flash between runs
 
@@ -137,8 +155,10 @@
 Options:
       --device <DEVICE>          Device profile id; an unknown id prints the available ones
       --flash <FLASH>            Full flash image to boot: a dump of a device (`esptool.py read_flash 0 ALL flash.bin`), or the output of `esptool.py merge_bin`. Its bootloader, partition table and application run as on the device. Without it the flash is erased and the ROM reports an invalid header, as a blank chip does. The image is only read: what the firmware writes to the flash is kept by --flash-file, or lost when the run ends
+      --firmware <FILE>          Application image to boot (the .bin of an ESP-IDF build or of a release, its first byte 0xE9): the flash is composed from it, as a device freshly flashed holds it. The bootloader inkemu ships for the device goes at offset 0, the partition table of the device profile at 0x8000, the application in the slot the bootloader starts (the factory application, else the first OTA slot), and every other byte is erased, the OTA data partition included. The file is only read. A whole flash image goes to --flash instead
+      --bootloader <FILE>        Bootloader to compose the flash with instead of the one inkemu ships for the device: the bootloader.bin of a build. It is checked against the device and written as it is
       --flash-file <FILE>        Keep the flash of the device in this file from one run to the next: a raw image of exactly the size of the flash. A run starts from the file when it exists (--flash is then refused unless --reset-flash is given) and writes it when it ends: at a limit, at the first Ctrl-C, at a --strict-mmio stop, when the CPU waits for ever. The file is replaced in one step, never written while the run goes on, so a run that is killed leaves the file of before. The new content goes through a temporary file beside it (.FILE.tmp, the name of FILE after a dot): what is found at that name (an interrupted save leaves its file there) is removed by the next run that starts, and by the save, which say so. A lock file beside it (FILE.lock) refuses a second run on the same file; it is left in place. A flash holds what the firmware keeps, network credentials among it: on Unix a file created here is readable and writable by its owner only, one that is replaced keeps its mode, and one that cannot be written is refused
-      --reset-flash              Start from the --flash image, or from an erased flash, although the flash file exists, and replace the file when the run ends
+      --reset-flash              Start from the --flash image, or from an erased flash, although the flash file exists, and replace the file when the run ends. With --firmware, start from the flash composed from it
       --max-cycles <MAX_CYCLES>  Stop when the CPU has executed this many cycles of its clock. The limit counts executed cycles, not device time: a sleeping CPU executes none, so a firmware that sleeps reaches it late, and one whose sleep nothing interrupts never does; --max-time bounds both
       --max-time <SECONDS>       Stop when this many seconds of device time have elapsed since power-on (a decimal number: 30, 2.5, 0.001). Device time goes on while the CPU sleeps and across resets
       --strict-mmio              Stop at the first access to a register the emulator does not model, 8/16-bit write to a peripheral register, operation the emulator cannot perform faithfully, access to flash that is not mapped, or read through a flash window that may differ from the device (exit code 3)
@@ -176,7 +196,7 @@
 
 The first run downloads the ROM (4.9 MB) from Espressif's `esp-rom-elfs` release `20260528` and checks its SHA-256 before using it. To work offline, extract the archive yourself and pass `--rom-dir <directory>`. `inkemu run` reads only the ROM of its device's chip revision from that directory: `esp32c3_rev3_rom.elf` for `--device x4` and `--device x3`. `inkemu rom fetch --all [--rom-dir <directory>]` fills a directory in advance, with a single download, with the three ROMs of the supported chips (`esp32c3_rev0_rom.elf`, `esp32c3_rev3_rom.elf` and `esp32c3_rev101_rom.elf`); `inkemu rom fetch --device x4` fetches just the one that device needs.
 
-Other commands: `inkemu inspect <app.bin>` describes an ESP-IDF application image and explains what is wrong when it refuses a file; `inkemu rvtest <elf>` runs a bare-metal RISC-V conformance test. For the pinned conformance suites and how to run riscv-arch-test against Sail, see [tools/conformance/README.md](tools/conformance/README.md).
+Other commands: `inkemu inspect <app.bin>` describes an ESP-IDF application image and explains what is wrong when it refuses a file; `inkemu licenses` prints the notices of the third-party parts of the binary; `inkemu rvtest <elf>` runs a bare-metal RISC-V conformance test. For the pinned conformance suites and how to run riscv-arch-test against Sail, see [tools/conformance/README.md](tools/conformance/README.md).
 
 ## Why
 
@@ -204,7 +224,7 @@
 
 ## Roadmap
 
-1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and the device's flash file, done; 1b-3c, a firmware given on its own (`--firmware`), is next; then 1b-3d, the serial consoles in both directions; 1b-3e, a community firmware boots; 1b-3f, deep sleep; 1b-4, the lockstep against QEMU.*
+1. RV32IMC core, minimal ESP32-C3, firmware loader, CLI: boot a firmware to its console logs. *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, done; 1b-3a, interrupts, timers, watchdogs and reset, done; 1b-3b, flash writes and the device's flash file, done; 1b-3c, a firmware given on its own (`--firmware`), done; 1b-3d, the serial consoles in both directions, is next; then 1b-3e, a community firmware boots; 1b-3f, deep sleep; 1b-4, the lockstep against QEMU.*
 2. X4 peripherals, desktop UI, e-ink rendering: **first visual milestone**
 3. X3 support (UC8253 panel, I2C sensors)
 4. Debugging, resource monitor, headless test runner
@@ -222,6 +242,7 @@
 - [ADR 0002](docs/adr/0002-flash-model-and-cache-windows.md): the flash model and the cache windows (French)
 - [ADR 0003](docs/adr/0003-contexte-des-peripheriques.md): the context every peripheral access receives, device time, and the three levels of reset (French)
 - [ADR 0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md): what an input of a run is, and the life of the flash file (French)
+- [ADR 0005](docs/adr/0005-bootloader-embarque.md): the 2nd-stage bootloader inkemu ships, chosen by the device profile and replaceable (French)
 
 ## Français
 
@@ -229,11 +250,13 @@
 
 **inkemu est un émulateur au niveau binaire d'appareils embarqués à écran e-ink.** Il exécute l'image réelle d'un firmware, sans la modifier, sur une réplique virtuelle du matériel : on peut vérifier ce qu'un firmware affiche et comment il se comporte avant de le flasher.
 
-**État : début du développement.** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète, la ROM charge le bootloader de second niveau d'ESP-IDF, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. La flash s'écrit comme sur l'appareil : le bootloader enregistre l'emplacement OTA qu'il a choisi, NVS garde ce que le firmware y range, et `--flash-file` conserve la flash d'un appareil d'une exécution à la suivante. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail. Viennent ensuite le firmware fourni seul (sous-projet 1b-3c), les consoles série dans les deux sens (1b-3d), un firmware communautaire (1b-3e), le sommeil profond (1b-3f) et le lockstep avec QEMU (1b-4).
+**État : début du développement.** inkemu démarre la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset. À partir d'une image flash complète, la ROM charge le bootloader de second niveau d'ESP-IDF, qui lance l'application ; celle-ci vit toute sa vie — ordonnanceur FreeRTOS, interruption de tick, timers, watchdogs — et, quand elle redémarre, la puce démarre de nouveau. La flash s'écrit comme sur l'appareil : le bootloader enregistre l'emplacement OTA qu'il a choisi, NVS garde ce que le firmware y range, et `--flash-file` conserve la flash d'un appareil d'une exécution à la suivante. Un `.bin` d'application fourni seul démarre aussi : inkemu compose la flash autour de lui avec le bootloader ESP-IDF qu'il embarque et la table de partitions de l'appareil. Le cœur RV32IMC est validé contre riscv-tests puis, avec RISCOF, contre le modèle de référence Sail. Viennent ensuite les consoles série dans les deux sens (1b-3d), un firmware communautaire (1b-3e), le sommeil profond (1b-3f) et le lockstep avec QEMU (1b-4).
 
 ### Construire et essayer
 
-`cargo build --release`, puis `target/release/inkemu run --device x4 --flash flash.bin --max-cycles 50000000`, ou `--max-time 5` pour s'arrêter après cinq secondes d'appareil. `--flash` attend une **image flash complète** : le dump d'un appareil (`esptool.py read_flash 0 ALL flash.bin`) ou la sortie d'`esptool.py merge_bin`. Le fichier peut être plus court que la flash de l'appareil ; le reste se lit comme effacé. Un fichier qui ne peut pas être la flash de l'appareil est refusé avant toute exécution, avec la cause et le remède (code de sortie 2) : un `.xota` chiffré, une application seule, une image pour une autre puce, une image qui déclare plus de flash que l'appareil n'en a, un fichier plus long que la flash, un fichier vide. Sans `--flash`, la flash est effacée et la ROM affiche `invalid header: 0xffffffff` en boucle, comme une puce vierge. Il n'y a pas encore d'option pour démarrer un `.bin` d'application seul (sous-projet 1b-3c).
+`cargo build --release`, puis `target/release/inkemu run --device x4 --flash flash.bin --max-cycles 50000000`, ou `--max-time 5` pour s'arrêter après cinq secondes d'appareil. `--flash` attend une **image flash complète** : le dump d'un appareil (`esptool.py read_flash 0 ALL flash.bin`) ou la sortie d'`esptool.py merge_bin`. Le fichier peut être plus court que la flash de l'appareil ; le reste se lit comme effacé. Un fichier qui ne peut pas être la flash de l'appareil est refusé avant toute exécution, avec la cause et le remède (code de sortie 2) : un `.xota` chiffré, une application seule, une image pour une autre puce, une image qui déclare plus de flash que l'appareil n'en a, un fichier plus long que la flash, un fichier vide. Sans `--flash` ni `--firmware`, la flash est effacée et la ROM affiche `invalid header: 0xffffffff` en boucle, comme une puce vierge.
+
+**Démarrer une image d'application.** `--firmware <fichier>` démarre un `.bin` d'application seul (une version d'un firmware communautaire, le `firmware.bin` d'une construction PlatformIO, le `.bin` d'une construction ESP-IDF) comme le ferait un appareil qu'on vient de flasher. inkemu compose d'abord la flash : à l'adresse 0 le bootloader de second niveau d'ESP-IDF v5.5.2 qu'il embarque (construit depuis des sources non modifiées, voir l'[ADR 0005](docs/adr/0005-bootloader-embarque.md)), à 0x8000 la table de partitions du profil (celle d'origine pour le X4), l'application dans l'emplacement que le bootloader lance (l'application d'usine, sinon le premier emplacement OTA), et tous les autres octets effacés. Le profil choisit le bootloader d'après sa puce ; `--bootloader <fichier>` en met un autre à sa place, le `bootloader.bin` de votre construction par exemple. Au premier démarrage, le bootloader affiche la table, `No factory image, trying OTA 0` et `Set actual ota_seq=1 in otadata[0]`. Les mêmes fichiers donnent la même flash, dont le SHA-256 est imprimé (`flash at start`). Un fichier qui ne peut pas démarrer est refusé avant l'exécution, avec la cause et le remède (code de sortie 2) : un fichier qui n'est pas une image ESP-IDF, un `.xota` chiffré, une image pour une autre puce, une application plus grande que son emplacement, une somme de contrôle ou une empreinte fausse, une image flash entière (elle va à `--flash`), un bootloader donné comme firmware ou une application donnée comme bootloader, une application ou un bootloader qui demande une révision de puce plus récente que celle de l'appareil. `--firmware` n'est que lu et, comme `--flash`, est refusé à côté d'un fichier de flash qui existe, sauf avec `--reset-flash`. `inkemu licenses` imprime les avis et les licences de ce que le binaire porte de tiers : les descriptions de registres et le bootloader.
 
 L'image de référence (`hello_world` d'ESP-IDF v5.5.2) vit toute sa vie : `Hello world!`, onze secondes de compte à rebours, `esp_restart()`, puis un second démarrage que la ROM annonce par `rst:0xc (RTC_SW_CPU_RST)`. Les secondes d'attente ne sont pas exécutées : le CPU dort dans `wfi` et le temps de l'appareil saute au prochain événement de timer, si bien que les onze secondes du compte à rebours prennent environ 0,05 s de temps d'hôte, et l'exécution entière de 50 millions de cycles du CPU environ 1,1 s (mesuré sur une machine de développement). Avec `--strict-mmio`, la même exécution ne touche aucun registre qu'inkemu ne modélise pas et ne prend aucune exception, redémarrage compris.
 
@@ -279,7 +302,7 @@
 
 ### Feuille de route
 
-1. Cœur RV32IMC, ESP32-C3 minimal, chargeur de firmware, CLI : démarrer un firmware jusqu'à ses journaux de console. *1a fait ; 1b-1, squelette du SoC et démarrage de la ROM, fait ; 1b-2, flash et bootloader, fait ; 1b-3a, interruptions, timers, watchdogs et reset, fait ; 1b-3b, écritures en flash et fichier de flash de l'appareil, fait ; 1b-3c, firmware fourni seul (`--firmware`), est le suivant ; puis 1b-3d, consoles série dans les deux sens ; 1b-3e, démarrage d'un firmware communautaire ; 1b-3f, sommeil profond ; 1b-4, lockstep avec QEMU.*
+1. Cœur RV32IMC, ESP32-C3 minimal, chargeur de firmware, CLI : démarrer un firmware jusqu'à ses journaux de console. *1a fait ; 1b-1, squelette du SoC et démarrage de la ROM, fait ; 1b-2, flash et bootloader, fait ; 1b-3a, interruptions, timers, watchdogs et reset, fait ; 1b-3b, écritures en flash et fichier de flash de l'appareil, fait ; 1b-3c, firmware fourni seul (`--firmware`), fait ; 1b-3d, consoles série dans les deux sens, est le suivant ; puis 1b-3e, démarrage d'un firmware communautaire ; 1b-3f, sommeil profond ; 1b-4, lockstep avec QEMU.*
 2. Périphériques du X4, interface de bureau, rendu e-ink : **premier jalon visuel**
 3. Prise en charge du X3 (dalle UC8253, capteurs I2C)
 4. Débogage, moniteur de ressources, exécution de tests sans écran
@@ -291,19 +314,20 @@
 
 ### Documentation
 
-La [spécification](docs/superpowers/specs/2026-10-11-inkemu-design.md), les ADR [0002](docs/adr/0002-flash-model-and-cache-windows.md) (modèle de la flash et fenêtres de cache) [0003](docs/adr/0003-contexte-des-peripheriques.md) (contexte des périphériques, temps de l'appareil et resets) et [0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md) (entrées d'une exécution et fichier de flash) sont en français ; la [recherche sur le matériel Xteink](docs/research/xteink-hardware.md) et la [table des registres acceptés sans effet](docs/registers-accepted-without-effect.md) sont en anglais.
+La [spécification](docs/superpowers/specs/2026-10-11-inkemu-design.md), les ADR [0002](docs/adr/0002-flash-model-and-cache-windows.md) (modèle de la flash et fenêtres de cache) [0003](docs/adr/0003-contexte-des-peripheriques.md) (contexte des périphériques, temps de l'appareil et resets) [0004](docs/adr/0004-entrees-d-une-execution-et-fichier-de-flash.md) (entrées d'une exécution et fichier de flash) et [0005](docs/adr/0005-bootloader-embarque.md) (bootloader embarqué) sont en français ; la [recherche sur le matériel Xteink](docs/research/xteink-hardware.md) et la [table des registres acceptés sans effet](docs/registers-accepted-without-effect.md) sont en anglais.
 
 ### Matériel tiers et licence
 
-La ROM masquée de l'ESP32-C3 est le code d'Espressif et n'est pas libre : inkemu ne la distribue jamais, il télécharge les fichiers ELF qu'Espressif publie sous Apache-2.0 dans `esp-rom-elfs` et les vérifie contre des empreintes épinglées. Le firmware de référence (bootloader ESP-IDF, Apache-2.0, et deux exemples ESP-IDF non modifiés, `hello_world` et `nvs_rw_value_cxx`, dans le domaine public) est construit dans Docker et vérifié de même ; aucun binaire de firmware n'est stocké dans ce dépôt. `third_party/espressif-svd/` contient une copie non modifiée du `esp32c3.svd` d'Espressif (Apache-2.0), dont `crates/svd-regs` est généré. Voir l'[ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md). inkemu est sous [licence Apache 2.0](LICENSE). Xteink est une marque de son propriétaire ; ce projet n'est affilié ni à Xteink ni à Espressif.
+La ROM masquée de l'ESP32-C3 est le code d'Espressif et n'est pas libre : inkemu ne la distribue jamais, il télécharge les fichiers ELF qu'Espressif publie sous Apache-2.0 dans `esp-rom-elfs` et les vérifie contre des empreintes épinglées. Le firmware de référence (bootloader ESP-IDF, Apache-2.0, et deux exemples ESP-IDF non modifiés, `hello_world` et `nvs_rw_value_cxx`, dans le domaine public) est construit dans Docker et vérifié de même. Le seul binaire tiers du dépôt est le bootloader qu'inkemu embarque, `third_party/esp-idf-bootloader/` (ESP-IDF v5.5.2, Apache-2.0, avec deux avis BSD-3-Clause, sa licence et sa provenance) : la CI vérifie qu'il est celui de la construction de référence ; aucun autre binaire de firmware n'est stocké. `third_party/espressif-svd/` contient une copie non modifiée du `esp32c3.svd` d'Espressif (Apache-2.0), dont `crates/svd-regs` est généré. Voir les ADR [0001](docs/adr/0001-espressif-rom-and-svd-licensing.md) et [0005](docs/adr/0005-bootloader-embarque.md) ; `inkemu licenses` imprime les avis. inkemu est sous [licence Apache 2.0](LICENSE). Xteink est une marque de son propriétaire ; ce projet n'est affilié ni à Xteink ni à Espressif.
 
 ## Third-party material
 
 - **Mask ROM.** The ESP32-C3 ROM is Espressif's code and is not open source. inkemu never ships it: it downloads the ROM ELF files that Espressif publishes under Apache-2.0 in [`esp-rom-elfs`](https://github.com/espressif/esp-rom-elfs) and verifies them against pinned hashes.
-- **Reference firmware.** The tests boot the stock ESP-IDF bootloader (Apache-2.0) and two unmodified ESP-IDF examples, `hello_world` and `nvs_rw_value_cxx` (public domain, `Unlicense OR CC0-1.0`). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository.
+- **Reference firmware.** The tests boot the stock ESP-IDF bootloader (Apache-2.0) and two unmodified ESP-IDF examples, `hello_world` and `nvs_rw_value_cxx` (public domain, `Unlicense OR CC0-1.0`). They are built in Docker by `tools/reference-firmware/build.sh` and checked against pinned hashes; no firmware binary is stored in this repository but the one below.
+- **The bootloader inkemu ships.** `third_party/esp-idf-bootloader/` holds the ESP-IDF v5.5.2 2nd-stage bootloader of the reference build (Apache-2.0, with two BSD-3-Clause notices), with its licence, its notice and its provenance; it is embedded in the binary, and the CI checks that it is the one the reference build produces. `inkemu licenses` prints the notices.
 - **Register descriptions.** `third_party/espressif-svd/` holds an unmodified copy of Espressif's `esp32c3.svd` (Apache-2.0), from which `crates/svd-regs` is generated.
 
-See [ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md).
+See [ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md) and [ADR 0005](docs/adr/0005-bootloader-embarque.md).
 
 ## License
 
````


````diff
--- a/crates/firmware/src/error.rs
+++ b/crates/firmware/src/error.rs
@@ -88,7 +88,7 @@
     )]
     FlashImageTooShort { len: usize },
     #[error(
-        "the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)"
+        "the file starts with an image but holds no partition table at offset 0x8000, so it is an application or a bootloader alone, not a full flash image; use an application with --firmware, or give --flash a flash dump or the output of `esptool.py merge_bin` (bootloader, partition table and application)"
     )]
     NoPartitionTable,
 }
````


Create `docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md`:

````markdown
# SP1b-3c — Note de passation

Plan : `docs/superpowers/plans/2026-10-13-sp1b-3c-firmware.md` (`--firmware` : une image d'application démarre derrière le bootloader qu'inkemu embarque). Décision durable : ADR 0005 (le bootloader de second étage, embarqué, choisi par le profil, remplaçable).

Les numéros de décision cités ici (C1 à C30) se résolvent dans le plan versionné, au tableau « Design decisions taken for this plan », pas dans l'ADR ni dans le code : C1 à C15 ont été donnés au plan, C16 à C30 pris pendant son écriture.

## Résultat

- **`inkemu run --device x4 --firmware hello_world.bin` démarre l'application seule**, derrière le bootloader ESP-IDF v5.5.2 qu'inkemu embarque et la table de partitions d'origine du X4 : en mode strict, toute la vie de l'application, son redémarrage et le second démarrage, sans accès non modélisé. Au premier démarrage, le bootloader imprime ses 26 lignes, celles de la capture QEMU de la même flash (`golden_hello_ota_boot.log`) aux deux différences connues près (révision de bloc eFuse v1.2, mode SPI DIO), dont la table du X4, `No factory image, trying OTA 0` et `Set actual ota_seq=1 in otadata[0]` ; au second, ni l'une ni l'autre de ces deux lignes.
- **La flash composée est celle de la construction de référence** : pour `hello_world`, `9ddacb60…11bc89`, l'image `flash_hello_ota.bin` du rapport TO §2 ; pour `nvs_rw_value_cxx`, `87832c6d…f28fec`, l'image épinglée `flash_nvs_ota.bin`. Le SHA-256 est imprimé (`flash at start`), et deux exécutions ont la même empreinte de trace : `c6b24a1cc226ed4a` pour `--firmware hello_world.bin --strict-mmio --max-cycles 40000001`, 11 403 054 µs d'appareil, 26 637 404 instructions, épinglée par un test. L'empreinte de la vie de l'image fusionnée, `10174cc2726c3d77`, ne change pas.
- **Le compteur de NVS continue d'une exécution à l'autre à partir de l'application seule** : une première exécution `--firmware nvs_rw_value_cxx.bin --flash-file x4.bin` écrit `otadata` et affiche `The value is not initialized yet!` ; une seconde, avec le fichier seul, démarre à froid et affiche `Restart counter = 1`.
- **Chaque refus est nommé avant le démarrage**, avec la cause puis le remède, et testé à travers la commande : un fichier qui n'est pas une image, un `.xota` chiffré, une image ESP32-S3 pour un X4, une application plus grande que son emplacement, une empreinte fausse, une image entière (« `--flash` »), un bootloader donné comme firmware (« `--bootloader` »), une application donnée comme bootloader (« `--firmware` »), un bootloader qui demande une puce plus récente (il la réinitialiserait sans fin).
- **`inkemu licenses`** imprime le `NOTICE` d'inkemu puis l'avis et la licence de chaque partie tierce du binaire ; le même texte vient de `boards::notices`, pour la page « À propos » d'une interface.
- **Tout passe par des bibliothèques** : composition, registre des bootloaders, contrôles et tables de partitions sont dans `firmware` et `boards`, avec des erreurs typées ; les deux crates se compilent pour WebAssembly, et la CI le vérifie. `emu-cli` lit les fichiers, appelle `DeviceProfile::compose_flash` et imprime.

## Ce que devient la note de SP1b-3b

Sa section « Reporté, par héritier » donnait quatre points à SP1b-3c ; ce plan les a tous faits :

- `--firmware` (tâche 9) ;
- l'ADR du bootloader embarqué, numéro 0005 (tâche 9) ;
- les tables de partitions dans les profils (tâche 7) ;
- les retouches de forme de `boards` : la section `# Errors` de `from_named_toml` nomme `MissingFlash`, et `ProfileError::Parse` ne répète plus le remède (tâche 1 ; un refus du crate garde le sien, un refus de l'analyseur TOML reçoit le remède général, une fois).

Aucun document ne donne plus à SP1b-3c ce qu'il n'a pas fait : la tâche 1 l'a vérifié, la tâche 11 aussi. Le commentaire de `accepted_without_effect.rs` ne cite plus SP1b-3c parmi les plans à venir sans ligne.

## Décisions prises pendant l'écriture du plan

Elles sont dans le tableau des décisions du plan, de C16 à C30. Les principales :

- **L'application va dans l'application d'usine, sinon dans `ota_0`** (C16, qui corrige C7). C7, tel qu'il a été donné au plan, disait l'inverse ; le code du bootloader dit celui-ci (`bootloader_utility.c`, `bootloader_utility_get_selected_boot_partition` : « Defaulting to factory image », sinon « No factory image, trying OTA 0 »). Les deux profils n'ont pas d'application d'usine : rien ne change pour eux.
- **Le bootloader livré est réécrit pour la taille de la flash, un bootloader donné ne l'est jamais** (C17). Un bootloader de l'utilisateur dont l'en-tête dit plus de flash que l'appareil est refusé ; moins, c'est un avertissement.
- **Une empreinte ajoutée fausse sur un bootloader donné est un avertissement** (C18), comme la ROM la traite sans démarrage sécurisé ; une somme de contrôle fausse reste un refus.
- **Le mode de flash n'est pas une clé du profil** (C19) : tous les profils sont en DIO, une constante de `boards` ; une clé viendra avec le premier appareil QIO ou octal.
- **La table de partitions est une suite de lignes `[[partition]]` du profil** (C20), obligatoire, compilée et jugée contre la taille de la flash au chargement ; les sous-types s'écrivent par leur nom, comme dans un CSV de `gen_esp32part.py`.
- **La révision de bloc eFuse que l'application doit accepter vient du modèle des eFuse** (C21, `Efuse::block_revision`, v1.2), pas d'une clé du profil.
- **Les applications de référence sont prises dans les images fusionnées épinglées** (C25) : aucune construction Docker de plus, aucun fichier ni variable d'environnement de plus ; chaque test les découpe à 0x10000 et vérifie leur SHA-256 contre `SHA256SUMS.inputs` et `SHA256SUMS.inputs-nvs`. Le README de `tools/reference-firmware` donne la commande `dd` pour le faire à la main.
- **La preuve de C1 « le fichier versionné est celui de la construction de référence » est un test ignoré du job `reference-boot`** (C26), qui compare les premiers octets de `flash_hello.bin` au bootloader embarqué, plutôt qu'une étape de script.
- **Les fichiers de licence sont dans `third_party/esp-idf-bootloader/`, le binaire dans `esp32c3/`** (C27) : une licence et un avis pour tous les bootloaders ESP-IDF ; celui de l'ESP32-S3 ajoutera son fichier, une ligne de provenance et les avis Tensilica et Cadence.
- **Les refus de `flash_state.rs` nomment l'option qui a donné le fichier** (C28) : `--flash`, `--firmware` ou `--bootloader`. C'est un changement de libellé, pas de logique : la dette que SP2 hérite n'a pas grandi. Le lecteur borné de `flash_file.rs` est partagé par `firmware_file.rs` au lieu d'être recopié.
- **Le refus d'une application donnée à `--flash` propose `--firmware`** (C29).
- **`inkemu inspect` reste tel qu'il est** (C30) : décrire une image entière ou un bootloader avec son identité revient à SP2 (voir plus bas).

## Ce que les tests prouvent, et ce qu'ils ne prouvent pas

Chaque comportement que le plan affirme nomme la mutation qui doit faire échouer son test ; chacune a été appliquée, et le test nommé a échoué.

- **La révision de puce d'un profil sous le minimum du bootloader livré** n'est testée qu'au niveau de la bibliothèque (`boards` et `firmware`), avec un profil v0.2 lu depuis un texte : les deux profils livrés sont v0.3, le minimum. À travers la commande, le refus de même nature est tenu par un bootloader donné qui demande v0.4.
- **Le registre ne peut pas mentir** : un test compare chaque entrée (puce, mode, révision minimale, version d'ESP-IDF, empreinte, longueur) à l'en-tête et aux octets qu'elle porte.
- **La branche « famille sans bootloader livré »** est tenue par la fonction de sélection appelée avec l'ESP32-S3 ; aucun profil ne l'atteint.
- **La réécriture de l'en-tête** n'est exercée sur une vraie image que pour 4 Mio (empreinte de la construction 4 Mio mesurée) ; les autres tailles suivent la même règle, sans construction de référence pour les comparer.
- Ce que la comparaison avec QEMU ne dit pas : les 26 lignes du bootloader sont comparées au journal QEMU, mais les lignes de l'application d'après ne sont vérifiées que par leurs jalons.

## Hypothèses nouvelles

`grep -rn "ASSUMPTION:" crates --include='*.rs'` rend toujours 86 lignes. Un marqueur s'ajoute dans un profil :

- `crates/boards/profiles/x3.toml` : les noms, les drapeaux et l'entrée MD5 de la table du X3 ne sont pas établis ; les noms sont ceux du X4, aucun drapeau, l'entrée MD5 telle que `gen_esp32part.py` l'écrit (table compilée `334c9553…ae7d5e`). Les décalages et les tailles sont ceux des appareils réels. (Un dump des 0x9000 premiers octets d'un X3.)

## Reporté, par héritier

- **SP1b-3d** : inchangé (réception et interruptions UART, format de trame, côté hôte de l'USB-Serial-JTAG, pilote de console commun).
- **SP1b-3e** (CrossPoint) : aucun test CrossPoint dans ce plan. Ce que le rapport SB §C a mesuré : CrossPoint 1.6.5 démarre sur inkemu derrière le bootloader livré (l'image de FW §K.1), avec les 27 lignes du bootloader et `Set actual ota_seq=1 in otadata[0]`, puis la même panique de watchdog d'interruption que sous QEMU dans l'attente du SAR ADC (`MEPC 0x4219b962`), un redémarrage et un second démarrage ; la seconde vie interroge `SPI2.CMD` au lieu de reproduire la panique, ce que QEMU fait : écart mesuré, non expliqué. Avec `--firmware`, l'application seule suffit désormais : `inkemu run --device x4 --firmware crosspoint-1.6.5-x3-x4.bin`. Le binaire de CrossPoint n'est ni embarqué ni téléchargé par défaut (FW §N).
- **SP1b-3f**, **SP1b-4**, **SP8** : inchangés.
- **SP2** :
  - installer une nouvelle application sur un fichier de flash qui existe, en gardant NVS (une mise à jour comparable à une OTA) : c'est une action de l'interface, qui doit choisir l'emplacement et l'état d'`otadata` ; `--firmware` est une image de départ, refusée à côté d'un fichier de flash qui existe sauf avec `--reset-flash` ;
  - déplacer `crates/emu-cli/src/flash_state.rs` dans un crate de bibliothèque (la dette que ce plan n'a pas fait grandir) ;
  - `inkemu inspect` d'une image entière ou d'un fichier de flash (bootloader et son identité, table, application) ; aujourd'hui `inspect` refuse une image entière et décrit un bootloader comme une « ESP-IDF application image » ;
  - les messages des refus nomment les options de la commande (`--flash`, `--firmware`, `--bootloader`) : une interface les rend par leur variante typée, avec ses propres mots ;
  - un profil lu depuis un fichier de l'utilisateur (aujourd'hui seuls les profils livrés sont accessibles depuis la commande).
- **SP7** (ESP32-S3) : le bootloader de l'ESP32-S3 (la même recette : 20 992 octets, `53765fcd…a023e`), son entrée du registre, ses avis Tensilica et Cadence, et la ROM S3 dans le manifeste.
- **SP9** (distribution) : les licences des bibliothèques Rust liées dans l'exécutable, que `inkemu licenses` ne liste pas encore ; Windows, inchangé.
- **Quand un profil le demandera** : un bootloader « silencieux » (niveau ERROR, retour arrière, validation sautée au réveil), seconde entrée du registre qu'un profil ou une interface choisirait ; une construction QIO ou octale, avec une clé du profil qui nomme le mode de flash.

## Lignes de la table des registres acceptés, par héritier

Inchangées : SP1b-3d 8, SP1b-3f 10, SP1b-4 28, SP2 26, SP5a 8, SP5b 2, SP8 22. Ce plan n'ajoute ni ne retire aucun registre.

## Tests et coût en CI

`cargo test --workspace` : 1 064 tests passent, 64 sont ignorés (18 de `real_rom`, 13 de `reference_boot`, 32 de `run_cli`, 1 de `inspect_cli`). La CI exécute les tests ignorés avec leur compte exact :

- job `rom-boot` : `real_rom` 18, les tests de la commande sans firmware de référence 24 (inchangés) ;
- job `reference-boot` : `reference_boot` 13 (inchangé), les tests de la commande sur le firmware de référence passent de 4 à 8.

Le job `check` construit aussi `firmware` et `boards` pour WebAssembly. Le cache du firmware de référence est recalculé une fois, parce que le README de `tools/reference-firmware` change : la construction Docker a lieu une fois dans la CI, pas sur la machine de développement.

Durées des tests ignorés, en version optimisée, sur la machine de développement (16 cœurs, charge moyenne de 2 à 4), telles que le lanceur de tests les affiche : `real_rom` 4,5 s, les 24 tests de la commande 5,9 s, `reference_boot` 21,3 s, les 8 tests de la commande sur le firmware de référence 11,3 s.

Aucune dépendance n'est ajoutée : `boards` dépend désormais de `firmware`, un crate de l'espace de travail ; `cargo deny check` passe.

## À reporter dans la spec

- **§5.1, entrées acceptées** : « la table de partitions choisie dans le profil (stock X4, stock X3, CrossPoint) » devient « la table du profil » : chaque profil en énonce une, et celle de CrossPoint est celle du X4, octet pour octet. L'application va dans l'application d'usine, sinon dans `ota_0` : ce que le bootloader démarre quand `otadata` est vierge. `otadata` reste vierge.
- **§5.1, refus** : s'ajoutent ceux de §10.3 critère 7 que `--firmware` tient : une application plus grande que son emplacement, une image entière donnée comme application, un bootloader donné comme application et l'inverse, une révision de puce ou de bloc eFuse hors des bornes de l'application, un bootloader qui demande une puce plus récente. `--firmware` est, comme `--flash`, une image de départ : refusée à côté d'un fichier de flash qui existe, sauf avec `--reset-flash`.
- **§3.3, profils** : « les tables de partitions proposées » devient une table par profil, en lignes `[[partition]]`, validée contre la taille de la flash au chargement.
- **§3.4** : inchangée ; l'ADR 0005 en est la mise en œuvre (un bootloader par famille de puce et de mode de flash, la taille de flash écrite dans l'en-tête).
- **§6.4, CLI** : `inkemu licenses` imprime les avis des parties tierces du binaire ; `--bootloader` remplace le bootloader livré.
````


- [ ] **Step 4: Run the tests**

```bash
cargo test -p emu-cli --test run_cli an_application_image_given_as_flash_is_refused > /tmp/t11.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t11.log
/usr/bin/git grep -n "SP1b-3c" -- crates > /tmp/t11-3c.txt; echo "exit=$?"
grep -rn "ASSUMPTION:" crates --include='*.rs' > /tmp/t11-a.txt; echo "exit=$?"; wc -l < /tmp/t11-a.txt
cargo test --workspace > /tmp/t11-ws.log 2>&1; echo "exit=$?"; grep "^test result" /tmp/t11-ws.log | awk '{p+=$4; i+=$8} END {print p, i}'
```

Expected: `exit=0`, `1 passed`; the grep finds nothing in `crates` (`exit=1`); `86` `ASSUMPTION:` lines, as at the base (the new one is in `x3.toml`); then `exit=0` and `1064 64` (tests passed and ignored in the workspace).

- [ ] **Step 5: Ignored tests, with the counts of the CI**

With `INKEMU_ROM_ELF_DIR`, `INKEMU_REF_FLASH`, `INKEMU_NVS_FLASH` and `INKEMU_NVS_OTA_FLASH` set (absolute paths):

```bash
tools/ci/run-ignored-tests.sh 18 --release -p soc-esp32c3 --test real_rom -- --ignored > /tmp/ign1.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 24 --release -p emu-cli --test run_cli -- --ignored --skip reference_firmware > /tmp/ign2.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 13 --release -p soc-esp32c3 --test reference_boot -- --ignored > /tmp/ign3.log 2>&1; echo "exit=$?"
tools/ci/run-ignored-tests.sh 8 --release -p emu-cli --test run_cli reference_firmware -- --ignored > /tmp/ign4.log 2>&1; echo "exit=$?"
```

Expected: four `exit=0`, with `18`, `24`, `13` and `8` passed. The trace hash `10174cc2726c3d77` of the `--flash` life is unchanged (`x4_runs_the_reference_firmware_along_its_pinned_trace`).

- [ ] **Step 6: Coverage, licences, WebAssembly**

```bash
cargo llvm-cov --workspace --fail-under-lines 80 > /tmp/cov.log 2>&1; echo "exit=$?"; grep "^TOTAL" /tmp/cov.log
cargo deny check > /tmp/deny.log 2>&1; echo "exit=$?"
cargo check --target wasm32-unknown-unknown -p emu-core -p svd-regs -p cpu-rv32 -p soc-esp32c3 -p firmware -p boards > /tmp/wasm.log 2>&1; echo "exit=$?"
```

Expected: `exit=0` three times; line coverage 98.16 % for the workspace (100 % for `application.rs`, `device.rs` and `notices.rs`, 99.44 % `compose.rs`, 99.08 % `bootloader.rs`, 98.61 % `boards/src/lib.rs`, 90.43 % `firmware_file.rs`).

- [ ] **Step 7: Mutations**

Each row: apply the change, run `cargo test -p emu-cli --test run_cli an_application_image_given_as_flash_is_refused`, see the test fail, undo, `touch` the file.

| # | Mutation | Test that fails |
|---|---|---|
| 1 | `error.rs`: `use an application with --firmware, or give --flash` → `use` | `an_application_image_given_as_flash_is_refused` |

- [ ] **Step 8: Format, lint, commit**

```bash
cargo fmt --all > /tmp/fmt.log 2>&1; echo "exit=$?"
cargo fmt --all --check > /tmp/fmtc.log 2>&1; echo "exit=$?"
cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "exit=$?"
/usr/bin/git add docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md
/usr/bin/git commit -m "docs: hand-over of SP1b-3c; the README boots an application image; --flash points an application to --firmware" -- README.md crates/emu-cli/tests/run_cli.rs crates/firmware/src/error.rs crates/soc-esp32c3/tests/accepted_without_effect.rs docs/superpowers/notes/2026-10-13-sp1b-3c-carryover.md
```

Expected: three `exit=0`, then the commit.
