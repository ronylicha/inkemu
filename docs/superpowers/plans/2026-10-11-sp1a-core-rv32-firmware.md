# SP1a — Fondations, cœur RV32IMC et lecteur de firmware — Plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Poser le workspace Rust d'inkemu, un cœur RV32IMC+Zicsr validé par les suites de conformité officielles, et un lecteur d'images firmware ESP-IDF exposé par `inkemu inspect`.

**Architecture:** `emu-core` définit les contrats (bus, temps virtuel en cycles, ordonnanceur déterministe, modèle de timing, journal d'événements). `cpu-rv32` est un interpréteur avec cache de décodage qui ne dépend que de ces contrats. `firmware` lit et valide les images ESP-IDF et génère les tables de partitions. `emu-cli` expose `inkemu inspect` et le lanceur de tests de conformité. Le SoC ESP32-C3 et le boot réel sont l'objet du plan SP1b.

**Tech Stack:** Rust 1.99.0 (édition 2024), `thiserror`, `sha2`, `md-5`, `clap` 4, `serde`/`toml`, `assert_cmd`/`predicates`, `goblin` (lecture ELF des tests), Docker (toolchain RISC-V et ESP-IDF pour les fixtures), GitHub Actions, `cargo-deny`, `cargo-llvm-cov`.

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3, §4.1, §4.2, §4.4, §5.1, §7, §8, §10).

## Global Constraints

- Toolchain épinglée : Rust **1.99.0**, édition **2024**, via `rust-toolchain.toml`.
- Licence du projet : **Apache-2.0** ; toute dépendance doit passer `cargo deny check licenses` (MIT, Apache-2.0, BSD-2/3, ISC, Unicode-3.0, Zlib autorisées ; GPL interdite dans les crates distribuées).
- QEMU et toute toolchain GPL ne servent **qu'aux tests** : jamais liés, jamais distribués.
- `emu-core` ne dépend d'aucune autre crate du workspace.
- Aucun `unwrap()`/`expect()` hors tests, sauf invariant documenté par un commentaire `// INVARIANT:`.
- Erreurs : un `enum` `thiserror` par crate ; chaque message dit **la cause et la correction**.
- Nombres magiques interdits : constantes nommées, avec la source (manuel, spec) en commentaire.
- Couverture ≥ 80 % par crate ; **100 %** sur le décodeur d'instructions et le parsing d'images.
- Déterminisme : aucune source d'aléa, d'horloge murale ou d'ordre d'itération non défini (`HashMap`) dans le chemin d'exécution émulé.
- Commits : messages `type(scope): description`, uniquement les chemins de la tâche (`git commit -- <paths>`).

## Review Focus

- **Image tronquée ou corrompue** (fichier coupé au milieu d'un segment, nombre de segments gonflé) : refus propre avec la position fautive, jamais de panic ni d'allocation géante → test `rejects_truncated_segment` et `rejects_absurd_segment_length` (Task 10).
- **Accès mémoire non aligné ou non mappé** : l'exception RISC-V exacte (`mcause` 1, 4, 5, 6, 7) avec `mtval` renseigné, pas une erreur hôte → tests `misaligned_load_traps_or_succeeds_by_policy` et `unmapped_load_traps` (Task 5). Un saut non aligné (cause 0) est impossible avec l'extension C.
- **Code automodifiant** (écriture dans une page déjà décodée) : l'instruction réécrite est exécutée, pas l'ancienne → test `store_into_decoded_page_invalidates` (Task 6).
- **Mot d'instruction illégal** (`0x0000_0000`, `0xFFFF_FFFF`, opcode réservé, C.ADDI4SPN à immédiat nul) : exception « instruction illégale » avec `mtval` = le mot → test `illegal_encodings_trap` (Task 4 + 5).
- **Événements simultanés dans l'ordonnanceur** : deux événements au même cycle sont servis dans l'ordre d'insertion, à chaque exécution → test `same_cycle_events_fifo` (Task 3).

---

## File Structure

```
Cargo.toml                         workspace, profils, dépendances partagées
rust-toolchain.toml                1.99.0 + rustfmt, clippy, llvm-tools
deny.toml                          politique de licences et d'avis de sécurité
.github/workflows/ci.yml           fmt, clippy, tests, deny, couverture
docs/adr/0001-espressif-rom-and-svd-licensing.md
crates/emu-core/src/
  lib.rs                           ré-exports
  time.rs                          Cycles, Frequency (conversion cycles ↔ ns)
  bus.rs                           trait Bus, AccessSize, BusError
  memory.rs                        Ram (bloc linéaire) — utilisé par les tests et les SoC
  scheduler.rs                     Scheduler déterministe (cycle, séquence)
  timing.rs                        trait TimingModel, InstrClass
  trace.rs                         TraceHash, empreinte de trace FNV-1a 64 (EventLog : SP1b)
crates/cpu-rv32/src/
  lib.rs                           ré-exports
  instr.rs                         enum Instr (forme décodée)
  decode.rs                        décodage 32 bits
  decode_c.rs                      expansion des instructions compressées (RVC)
  csr.rs                           banque de CSR machine
  trap.rs                          Exception, codes mcause
  hart.rs                          état du hart + step()
  exec.rs                          sémantique de chaque Instr
  dcache.rs                        cache de décodage par page
  timing.rs                        DatasheetTiming (coûts par classe)
  asm.rs                           mini-assembleur, tests uniquement
crates/firmware/src/
  lib.rs
  error.rs                         FirmwareError
  chip.rs                          ChipId, FlashSize, SpiMode
  image.rs                         parsing/validation d'image ESP-IDF
  app_desc.rs                      esp_app_desc_t
  partition.rs                     table de partitions (génération + parsing)
  builder.rs                       constructeur d'images (feature test-support)
crates/emu-cli/src/
  lib.rs                           modules partagés binaire / tests d'intégration
  main.rs                          clap : inspect, rvtest
  inspect.rs                       rendu texte de `inspect`
  rvtest.rs                        machine de test minimale (riscv-tests / arch-test)
tests/fixtures/riscv-tests/        ELF riscv-tests générés + SHA256SUMS (Task 8)
tests/fixtures/partitions/         CSV + table de référence ESP-IDF (Task 11)
tools/docker/riscv-gcc/            toolchain riscv64-unknown-elf (tests uniquement)
tools/docker/riscof/               RISCOF + Sail + riscv-arch-test (tests uniquement)
tools/docker/esp-idf/              ESP-IDF épinglé (fixtures uniquement)
tools/conformance/                 versions.env, scripts des suites, plugin RISCOF, liste d'exclusion
tools/fixtures/                    régénération des fixtures ESP-IDF
```

---

### Task 1: Workspace, toolchain et CI

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `.github/workflows/ci.yml`
- Create: `crates/emu-core/Cargo.toml`, `crates/emu-core/src/lib.rs`
- Create: `crates/cpu-rv32/Cargo.toml`, `crates/cpu-rv32/src/lib.rs`
- Create: `crates/firmware/Cargo.toml`, `crates/firmware/src/lib.rs`
- Create: `crates/emu-cli/Cargo.toml`, `crates/emu-cli/src/main.rs`

**Interfaces:**
- Produces: un workspace qui compile, avec 4 crates vides ; binaire `inkemu`.

- [ ] **Step 1: Épingler la toolchain**

`rust-toolchain.toml` :
```toml
[toolchain]
channel = "1.99.0"
components = ["rustfmt", "clippy", "llvm-tools-preview"]
profile = "minimal"
```

- [ ] **Step 2: Créer le workspace**

`Cargo.toml` :
```toml
[workspace]
resolver = "3"
members = ["crates/emu-core", "crates/cpu-rv32", "crates/firmware", "crates/emu-cli"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.99"
license = "Apache-2.0"
repository = "https://github.com/ronylicha/inkemu"

[workspace.dependencies]
emu-core = { path = "crates/emu-core" }
cpu-rv32 = { path = "crates/cpu-rv32" }
firmware = { path = "crates/firmware" }
thiserror = "2"
sha2 = "0.10"
md-5 = "0.10"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
goblin = "0.9"
assert_cmd = "2"
predicates = "3"

[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"
expect_used = "deny"
module_name_repetitions = "allow"

[profile.release]
lto = "thin"
codegen-units = 1
debug = "line-tables-only"
```

Chaque crate (`crates/<nom>/Cargo.toml`) suit ce modèle (exemple `emu-core`) :
```toml
[package]
name = "emu-core"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
thiserror.workspace = true
```
- `cpu-rv32` : dépendances `emu-core.workspace = true`, `thiserror.workspace = true`.
- `firmware` : `thiserror`, `sha2`, `md-5`, `serde` (workspace).
- `emu-cli` : `[[bin]] name = "inkemu" path = "src/main.rs"` ; dépendances `emu-core`, `cpu-rv32`, `firmware`, `clap`, `goblin` ; `[dev-dependencies]` `assert_cmd`, `predicates`.

Les `lib.rs` contiennent une seule ligne de doc (`//! ...`). `main.rs` :
```rust
//! inkemu command-line interface.
fn main() {}
```

- [ ] **Step 3: Vérifier que le workspace compile**

Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: succès, aucun warning.

- [ ] **Step 4: Politique de licences**

`deny.toml` :
```toml
[graph]
all-features = true

[licenses]
allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib"]
confidence-threshold = 0.9

[bans]
multiple-versions = "warn"
wildcards = "deny"

[advisories]
yanked = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "deny"
```
Run: `cargo install cargo-deny --locked && cargo deny check`
Expected: `advisories ok, bans ok, licenses ok, sources ok`.

- [ ] **Step 5: CI GitHub Actions**

`.github/workflows/ci.yml` :
```yaml
name: CI
on:
  push: { branches: [main] }
  pull_request:
permissions:
  contents: read
jobs:
  check:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.99.0", components: "rustfmt, clippy, llvm-tools-preview" }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
      - uses: EmbarkStudios/cargo-deny-action@v2
  coverage:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.99.0", components: "llvm-tools-preview" }
      - uses: taiki-e/install-action@cargo-llvm-cov
      - run: cargo llvm-cov --workspace --fail-under-lines 80
```

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml rust-toolchain.toml deny.toml .github crates
git commit -m "chore: bootstrap Rust 1.99 workspace, lint policy and CI" -- Cargo.toml rust-toolchain.toml deny.toml .github crates
```

---

### Task 2: ADR 0001 — licences de la ROM et des SVD Espressif

Cette tâche ne produit pas de code ; elle produit la décision dont dépend SP1b. Le livrable est rejetable seul : si les sources ne sont pas citées, l'ADR est refusé.

**Files:**
- Create: `docs/adr/0001-espressif-rom-and-svd-licensing.md`

**Interfaces:**
- Produces: décision `embedded` ou `download-on-first-run` pour la ROM ESP32-C3 et ESP32-S3, avec URL et SHA-256 de référence ; décision équivalente pour les SVD.

- [ ] **Step 1: Collecter les licences**

Run :
```bash
gh api repos/espressif/esp-rom-elfs/license -q '.license.spdx_id' ; \
gh api repos/espressif/esp-rom-elfs/releases/latest -q '.tag_name, (.assets[].name)' ; \
gh api repos/espressif/svd/license -q '.license.spdx_id' ; \
gh api repos/espressif/qemu/releases/latest -q '.tag_name, (.assets[].name)' | grep -i -E 'tag|rom|esp-develop'
```
Lire aussi le README et le fichier `LICENSE` de `espressif/esp-rom-elfs` (`gh api repos/espressif/esp-rom-elfs/contents/LICENSE -q .content | base64 -d`), et chercher toute mention des fichiers `*.bin` de ROM.

- [ ] **Step 2: Rédiger l'ADR**

Structure obligatoire :
```markdown
# ADR 0001 — Licences de la ROM et des fichiers SVD Espressif

- Date : <date du jour>
- Statut : accepté

## Contexte
La ROM masquée de l'ESP32-C3/S3 est indispensable (le bootloader et l'application appellent ses fonctions).
inkemu est publié sous Apache-2.0.

## Faits constatés
| Élément | Dépôt / release | Licence SPDX | Fichiers ROM binaires présents ? | Source (URL) |
|---|---|---|---|---|

## Décision
- ROM : `embedded` | `download-on-first-run` — justification.
- URL de téléchargement exacte, SHA-256 des fichiers ROM C3 et S3.
- SVD : `vendored` | `download-at-build` — justification.

## Conséquences
- Emplacement du cache local (`<data_dir>/inkemu/rom/`), vérification d'empreinte, message d'erreur si absent ou altéré.
```
Si aucune licence explicite n'autorise la redistribution, la décision est obligatoirement `download-on-first-run`.

- [ ] **Step 3: Calculer les empreintes**

Télécharger les fichiers ROM retenus dans le scratchpad, puis `sha256sum`, et reporter les valeurs dans l'ADR. Ne pas committer les binaires si la décision est `download-on-first-run`.

- [ ] **Step 4: Commit**

```bash
git add docs/adr/0001-espressif-rom-and-svd-licensing.md
git commit -m "docs(adr): decide Espressif ROM and SVD distribution" -- docs/adr/0001-espressif-rom-and-svd-licensing.md
```

---

### Task 3: `emu-core` — temps, bus, mémoire, ordonnanceur, timing, empreinte de trace

Le journal d'événements enregistrable (`EventLog`) n'est pas utile tant qu'aucune entrée extérieure n'existe : il est livré en SP1b. Cette tâche livre `TraceHash`, qui sert déjà au test de déterminisme du cœur.
**Files:**
- Create: `crates/emu-core/src/{time.rs,bus.rs,memory.rs,scheduler.rs,timing.rs,trace.rs}`
- Modify: `crates/emu-core/src/lib.rs`

**Interfaces:**
- Produces (utilisés par toutes les tâches suivantes) :
  - `Cycles(pub u64)` : `Copy + Ord + Hash`, `Cycles::ZERO`, `impl Add<u64>`, `fn get(self) -> u64`.
  - `Frequency::from_hz(u64) -> Option<Frequency>` (refuse 0), `fn hz(&self) -> u64`, `fn cycles_to_nanos(&self, Cycles) -> u64`.
  - `AccessSize { Byte, Half, Word }` + `fn bytes(self) -> u32`.
  - `AccessKind { Fetch, Load, Store }`.
  - `ReadOutcome { value: u32, stall: u32 }` ; `BusError::{Unmapped { addr, kind }, Misaligned { addr, size }}`.
  - `trait Bus { fn read(&mut self, addr: u32, size: AccessSize, kind: AccessKind) -> Result<ReadOutcome, BusError>; fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32 /*stall*/, BusError>; fn page_version(&self, addr: u32) -> u64; }`
  - `Ram::new(base: u32, len: usize) -> Ram` (implémente `Bus`), `fn load(&mut self, addr: u32, bytes: &[u8]) -> Result<(), BusError>`, `fn bytes(&self, addr: u32, len: usize) -> Result<&[u8], BusError>`, `PAGE_SHIFT = 12`.
  - `Scheduler<E>` : `new()`, `schedule(at: Cycles, ev: E) -> EventId`, `cancel(EventId)`, `next_due(&self) -> Option<Cycles>`, `pop_due(&mut self, now: Cycles) -> Option<(Cycles, E)>`.
  - `InstrClass { Alu, Mul, Div, Load, Store, BranchTaken, BranchNotTaken, Jump, Csr, System }` ; `trait TimingModel { fn name(&self) -> &'static str; fn is_calibrated(&self) -> bool; fn instr_cycles(&self, class: InstrClass) -> u32; }`.
  - `TraceHash` : `new()`, `mix_u32(&mut self, u32)`, `mix_u64(&mut self, u64)`, `finish(&self) -> u64` (FNV-1a 64 bits).

- [ ] **Step 1: Écrire les tests (échouent faute de code)**

À la fin de chaque fichier, un module `#[cfg(test)] mod tests`. Contenu exact :

`time.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_frequency_is_rejected() { assert!(Frequency::from_hz(0).is_none()); }
    #[test]
    fn cycles_convert_to_nanos_without_overflow() {
        let f = Frequency::from_hz(160_000_000).unwrap();
        assert_eq!(f.cycles_to_nanos(Cycles(160)), 1_000);
        // u64::MAX cycles at 160 MHz ≈ 1.15e20 ns, beyond u64: the conversion saturates.
        assert_eq!(f.cycles_to_nanos(Cycles(u64::MAX)), u64::MAX);
    }
}
```

`memory.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{AccessKind, AccessSize, Bus, BusError};
    #[test]
    fn little_endian_round_trip() {
        let mut ram = Ram::new(0x1000, 16);
        ram.write(0x1000, AccessSize::Word, 0x1122_3344).unwrap();
        assert_eq!(ram.bytes(0x1000, 4).unwrap(), &[0x44, 0x33, 0x22, 0x11]);
        assert_eq!(ram.read(0x1002, AccessSize::Half, AccessKind::Load).unwrap().value, 0x1122);
    }
    #[test]
    fn access_outside_range_is_unmapped() {
        let mut ram = Ram::new(0x1000, 16);
        assert_eq!(ram.read(0x100E, AccessSize::Word, AccessKind::Load),
                   Err(BusError::Unmapped { addr: 0x100E, kind: AccessKind::Load }));
        assert_eq!(ram.read(0x0FFF, AccessSize::Byte, AccessKind::Fetch),
                   Err(BusError::Unmapped { addr: 0x0FFF, kind: AccessKind::Fetch }));
    }
    #[test]
    fn write_bumps_page_version_only_for_touched_page() {
        let mut ram = Ram::new(0, 3 * 4096);
        let v0 = ram.page_version(0);
        let v1 = ram.page_version(4096);
        ram.write(10, AccessSize::Byte, 1).unwrap();
        assert_ne!(ram.page_version(0), v0);
        assert_eq!(ram.page_version(4096), v1);
    }
    #[test]
    fn write_spanning_two_pages_bumps_both() {
        let mut ram = Ram::new(0, 2 * 4096);
        let (a, b) = (ram.page_version(0), ram.page_version(4096));
        ram.write(4094, AccessSize::Word, 0xFFFF_FFFF).unwrap();
        assert_ne!(ram.page_version(0), a);
        assert_ne!(ram.page_version(4096), b);
    }
}
```
`Ram` ne vérifie **pas** l'alignement : c'est le CPU qui décide (le C3 lève une exception, d'autres cœurs non). D'où le test `write_spanning_two_pages_bumps_both`.

`scheduler.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pops_in_cycle_order() {
        let mut s = Scheduler::new();
        s.schedule(Cycles(30), 'c'); s.schedule(Cycles(10), 'a'); s.schedule(Cycles(20), 'b');
        assert_eq!(s.next_due(), Some(Cycles(10)));
        let order: Vec<char> = std::iter::from_fn(|| s.pop_due(Cycles(100)).map(|(_, e)| e)).collect();
        assert_eq!(order, vec!['a', 'b', 'c']);
    }
    #[test]
    fn same_cycle_events_fifo() {
        let mut s = Scheduler::new();
        for e in 0..100 { s.schedule(Cycles(5), e); }
        let order: Vec<i32> = std::iter::from_fn(|| s.pop_due(Cycles(5)).map(|(_, e)| e)).collect();
        assert_eq!(order, (0..100).collect::<Vec<_>>());
    }
    #[test]
    fn not_due_yet_returns_none() {
        let mut s = Scheduler::new();
        s.schedule(Cycles(50), ());
        assert!(s.pop_due(Cycles(49)).is_none());
        assert!(s.pop_due(Cycles(50)).is_some());
    }
    #[test]
    fn cancelled_event_never_fires() {
        let mut s = Scheduler::new();
        let id = s.schedule(Cycles(1), "x");
        s.schedule(Cycles(2), "y");
        s.cancel(id);
        assert_eq!(s.next_due(), Some(Cycles(2)));
        assert_eq!(s.pop_due(Cycles(10)).map(|(_, e)| e), Some("y"));
        assert!(s.pop_due(Cycles(10)).is_none());
    }
}
```

`trace.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_hash_is_fnv_offset_basis() { assert_eq!(TraceHash::new().finish(), 0xcbf2_9ce4_8422_2325); }
    #[test]
    fn order_matters() {
        let mut a = TraceHash::new(); a.mix_u32(1); a.mix_u32(2);
        let mut b = TraceHash::new(); b.mix_u32(2); b.mix_u32(1);
        assert_ne!(a.finish(), b.finish());
    }
}
```

- [ ] **Step 2: Lancer les tests pour constater l'échec**

Run: `cargo test -p emu-core`
Expected: erreurs de compilation (types inexistants).

- [ ] **Step 3: Implémenter**

`time.rs` :
```rust
//! Virtual time, measured in SoC clock cycles.
use std::num::NonZeroU64;
use std::ops::Add;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A point in virtual time, counted in cycles of the emulated SoC clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cycles(pub u64);

impl Cycles {
    pub const ZERO: Cycles = Cycles(0);
    #[must_use]
    pub const fn get(self) -> u64 { self.0 }
}

impl Add<u64> for Cycles {
    type Output = Cycles;
    fn add(self, rhs: u64) -> Cycles { Cycles(self.0.saturating_add(rhs)) }
}

/// A clock frequency; never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frequency(NonZeroU64);

impl Frequency {
    #[must_use]
    pub fn from_hz(hz: u64) -> Option<Frequency> { NonZeroU64::new(hz).map(Frequency) }
    #[must_use]
    pub fn hz(&self) -> u64 { self.0.get() }
    /// Converts a cycle count to nanoseconds, saturating at `u64::MAX`.
    #[must_use]
    pub fn cycles_to_nanos(&self, cycles: Cycles) -> u64 {
        let nanos = u128::from(cycles.0) * NANOS_PER_SECOND / u128::from(self.hz());
        u64::try_from(nanos).unwrap_or(u64::MAX)
    }
}
```

`bus.rs` :
```rust
//! The memory-mapped bus seen by a CPU core.
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessSize { Byte, Half, Word }

impl AccessSize {
    #[must_use]
    pub const fn bytes(self) -> u32 { match self { AccessSize::Byte => 1, AccessSize::Half => 2, AccessSize::Word => 4 } }
}

/// Why the bus is accessed; peripherals and fault reporting need the distinction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind { Fetch, Load, Store }

/// A completed read and the wait states it cost on top of the instruction's base cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadOutcome { pub value: u32, pub stall: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum BusError {
    #[error("no device is mapped at {addr:#010x} ({kind:?}); check the device memory map")]
    Unmapped { addr: u32, kind: AccessKind },
    #[error("misaligned {size:?} access at {addr:#010x}")]
    Misaligned { addr: u32, size: AccessSize },
}

/// Reads may have side effects on peripherals, hence `&mut self`.
pub trait Bus {
    /// # Errors
    /// Returns [`BusError`] when nothing answers at `addr`.
    fn read(&mut self, addr: u32, size: AccessSize, kind: AccessKind) -> Result<ReadOutcome, BusError>;
    /// Returns the stall cycles of the write.
    /// # Errors
    /// Returns [`BusError`] when nothing answers at `addr`.
    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError>;
    /// A counter that changes whenever the 4 KiB page containing `addr` may hold new bytes.
    /// Decode caches compare it to detect self-modifying code and DMA writes.
    fn page_version(&self, addr: u32) -> u64;
}
```

`memory.rs` :
```rust
//! A linear block of RAM, little-endian, with per-page write versions.
use crate::bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};

/// Page granularity used for code-invalidation tracking.
pub const PAGE_SHIFT: u32 = 12;

#[derive(Debug, Clone)]
pub struct Ram { base: u32, data: Vec<u8>, page_versions: Vec<u64>, next_version: u64 }

impl Ram {
    #[must_use]
    pub fn new(base: u32, len: usize) -> Ram {
        let pages = len.div_ceil(1 << PAGE_SHIFT);
        Ram { base, data: vec![0; len], page_versions: vec![0; pages], next_version: 1 }
    }

    fn offset(&self, addr: u32, len: u32, kind: AccessKind) -> Result<usize, BusError> {
        let start = addr.checked_sub(self.base).ok_or(BusError::Unmapped { addr, kind })? as usize;
        let end = start.checked_add(len as usize).ok_or(BusError::Unmapped { addr, kind })?;
        if end > self.data.len() { return Err(BusError::Unmapped { addr, kind }); }
        Ok(start)
    }

    fn bump_pages(&mut self, start: usize, len: usize) {
        let first = start >> PAGE_SHIFT;
        let last = (start + len - 1) >> PAGE_SHIFT;
        for page in &mut self.page_versions[first..=last] { *page = self.next_version; }
        self.next_version += 1;
    }

    /// Copies `bytes` at `addr` (used to load images); bumps page versions.
    /// # Errors
    /// [`BusError::Unmapped`] if the range does not fit.
    pub fn load(&mut self, addr: u32, bytes: &[u8]) -> Result<(), BusError> {
        if bytes.is_empty() { return Ok(()); }
        let len = u32::try_from(bytes.len()).map_err(|_| BusError::Unmapped { addr, kind: AccessKind::Store })?;
        let start = self.offset(addr, len, AccessKind::Store)?;
        self.data[start..start + bytes.len()].copy_from_slice(bytes);
        self.bump_pages(start, bytes.len());
        Ok(())
    }

    /// # Errors
    /// [`BusError::Unmapped`] if the range does not fit.
    pub fn bytes(&self, addr: u32, len: usize) -> Result<&[u8], BusError> {
        let len32 = u32::try_from(len).map_err(|_| BusError::Unmapped { addr, kind: AccessKind::Load })?;
        let start = self.offset(addr, len32, AccessKind::Load)?;
        Ok(&self.data[start..start + len])
    }
}

impl Bus for Ram {
    fn read(&mut self, addr: u32, size: AccessSize, kind: AccessKind) -> Result<ReadOutcome, BusError> {
        let start = self.offset(addr, size.bytes(), kind)?;
        let mut word = [0u8; 4];
        word[..size.bytes() as usize].copy_from_slice(&self.data[start..start + size.bytes() as usize]);
        Ok(ReadOutcome { value: u32::from_le_bytes(word), stall: 0 })
    }

    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        let n = size.bytes() as usize;
        let start = self.offset(addr, size.bytes(), AccessKind::Store)?;
        self.data[start..start + n].copy_from_slice(&value.to_le_bytes()[..n]);
        self.bump_pages(start, n);
        Ok(0)
    }

    fn page_version(&self, addr: u32) -> u64 {
        addr.checked_sub(self.base)
            .and_then(|off| self.page_versions.get((off >> PAGE_SHIFT) as usize))
            .copied()
            .unwrap_or(0)
    }
}
```

`scheduler.rs` :
```rust
//! Deterministic event queue: ordered by cycle, then by insertion order.
use crate::time::Cycles;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeSet, BinaryHeap};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventId(u64);

#[derive(Debug)]
struct Entry<E> { at: Cycles, id: EventId, event: E }

impl<E> PartialEq for Entry<E> { fn eq(&self, o: &Self) -> bool { self.key() == o.key() } }
impl<E> Eq for Entry<E> {}
impl<E> PartialOrd for Entry<E> { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }
impl<E> Ord for Entry<E> { fn cmp(&self, o: &Self) -> Ordering { self.key().cmp(&o.key()) } }
impl<E> Entry<E> { fn key(&self) -> (Cycles, EventId) { (self.at, self.id) } }

#[derive(Debug)]
pub struct Scheduler<E> { heap: BinaryHeap<Reverse<Entry<E>>>, cancelled: BTreeSet<EventId>, next_id: u64 }

impl<E> Default for Scheduler<E> { fn default() -> Self { Self::new() } }

impl<E> Scheduler<E> {
    #[must_use]
    pub fn new() -> Self { Scheduler { heap: BinaryHeap::new(), cancelled: BTreeSet::new(), next_id: 0 } }

    pub fn schedule(&mut self, at: Cycles, event: E) -> EventId {
        let id = EventId(self.next_id);
        self.next_id += 1;
        self.heap.push(Reverse(Entry { at, id, event }));
        id
    }

    pub fn cancel(&mut self, id: EventId) { self.cancelled.insert(id); }

    fn drop_cancelled_head(&mut self) {
        while let Some(Reverse(head)) = self.heap.peek() {
            if !self.cancelled.remove(&head.id) { break; }
            self.heap.pop();
        }
    }

    #[must_use]
    pub fn next_due(&self) -> Option<Cycles> {
        self.heap.iter().filter(|Reverse(e)| !self.cancelled.contains(&e.id)).map(|Reverse(e)| e.at).min()
    }

    pub fn pop_due(&mut self, now: Cycles) -> Option<(Cycles, E)> {
        self.drop_cancelled_head();
        match self.heap.peek() {
            Some(Reverse(head)) if head.at <= now => self.heap.pop().map(|Reverse(e)| (e.at, e.event)),
            _ => None,
        }
    }
}
```
`next_due` parcourt le tas pour ignorer les annulés sans muter ; le coût est acceptable car la file reste petite (quelques dizaines d'événements). Si un profilage ultérieur le contredit, on passera à un nettoyage paresseux.

`timing.rs` :
```rust
//! Pluggable timing models (spec §1.3, §4.2).

/// Coarse instruction categories that timing models assign costs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InstrClass { Alu, Mul, Div, Load, Store, BranchTaken, BranchNotTaken, Jump, Csr, System }

/// A source of instruction costs. Memory wait states come from the bus, not from here.
pub trait TimingModel: std::fmt::Debug {
    fn name(&self) -> &'static str;
    /// `false` until the model has been fitted against measurements on real hardware.
    fn is_calibrated(&self) -> bool;
    fn instr_cycles(&self, class: InstrClass) -> u32;
}
```

`trace.rs` :
```rust
//! Order-sensitive 64-bit FNV-1a hash used to prove that two runs executed identically.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraceHash(u64);

impl Default for TraceHash { fn default() -> Self { Self::new() } }

impl TraceHash {
    #[must_use]
    pub const fn new() -> Self { TraceHash(FNV_OFFSET_BASIS) }
    pub fn mix_u32(&mut self, v: u32) { self.mix_bytes(&v.to_le_bytes()); }
    pub fn mix_u64(&mut self, v: u64) { self.mix_bytes(&v.to_le_bytes()); }
    fn mix_bytes(&mut self, bytes: &[u8]) {
        for b in bytes { self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(FNV_PRIME); }
    }
    #[must_use]
    pub const fn finish(&self) -> u64 { self.0 }
}
```

`lib.rs` :
```rust
//! Core contracts shared by every inkemu crate: virtual time, bus, scheduler, timing, tracing.
pub mod bus;
pub mod memory;
pub mod scheduler;
pub mod time;
pub mod timing;
pub mod trace;

pub use bus::{AccessKind, AccessSize, Bus, BusError, ReadOutcome};
pub use memory::{Ram, PAGE_SHIFT};
pub use scheduler::{EventId, Scheduler};
pub use time::{Cycles, Frequency};
pub use timing::{InstrClass, TimingModel};
pub use trace::TraceHash;
```
Les tests utilisent `unwrap()` : ajouter `#![cfg_attr(test, allow(clippy::unwrap_used))]` en tête de `lib.rs`.

- [ ] **Step 4: Lancer les tests**

Run: `cargo test -p emu-core && cargo clippy -p emu-core --all-targets -- -D warnings`
Expected: tous les tests PASS, aucun warning.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(core): virtual time, bus, RAM, deterministic scheduler, timing and trace contracts" -- crates/emu-core
```

---

### Task 4: `cpu-rv32` — forme décodée et décodeur RV32IMC

Référence : *The RISC-V Instruction Set Manual, Volume I: Unprivileged ISA* (chapitres RV32I, M, C — « RVC opcode map »). Les instructions compressées sont **développées** vers les mêmes variantes `Instr` que les instructions 32 bits ; seul `len` change. Ainsi l'exécution n'existe qu'une fois.

**Files:**
- Create: `crates/cpu-rv32/src/instr.rs`, `crates/cpu-rv32/src/decode.rs`, `crates/cpu-rv32/src/decode_c.rs`
- Modify: `crates/cpu-rv32/src/lib.rs`

**Interfaces:**
- Produces :
  - `type Reg = u8` (0..=31).
  - `enum AluOp { Add, Sub, Sll, Slt, Sltu, Xor, Srl, Sra, Or, And }`, `enum MulOp { Mul, Mulh, Mulhsu, Mulhu, Div, Divu, Rem, Remu }`, `enum BranchOp { Eq, Ne, Lt, Ge, Ltu, Geu }`, `enum LoadOp { B, H, W, Bu, Hu }`, `enum StoreOp { B, H, W }`, `enum CsrOp { Rw, Rs, Rc }` — tous `Clone + Copy + Debug + PartialEq + Eq`.
  - `enum Instr` (variantes ci-dessous), `struct Decoded { instr: Instr, len: u8, raw: u32 }` (`raw` = mot 32 bits, ou parcelle 16 bits pour une compressée ; il devient `mtval` des instructions illégales).
  - `fn decode(raw: u32) -> Decoded` : si `raw & 0b11 != 0b11`, seuls les 16 bits de poids faible sont lus (instruction compressée, `len = 2`), sinon `len = 4`.

- [ ] **Step 1: Définir `Instr`**

`instr.rs` :
```rust
//! Decoded instruction form shared by the 32-bit and compressed decoders.
pub type Reg = u8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum AluOp { Add, Sub, Sll, Slt, Sltu, Xor, Srl, Sra, Or, And }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum MulOp { Mul, Mulh, Mulhsu, Mulhu, Div, Divu, Rem, Remu }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum BranchOp { Eq, Ne, Lt, Ge, Ltu, Geu }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum LoadOp { B, H, W, Bu, Hu }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum StoreOp { B, H, W }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum CsrOp { Rw, Rs, Rc }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instr {
    Lui { rd: Reg, imm: u32 },
    Auipc { rd: Reg, imm: u32 },
    Jal { rd: Reg, offset: i32 },
    Jalr { rd: Reg, rs1: Reg, offset: i32 },
    Branch { op: BranchOp, rs1: Reg, rs2: Reg, offset: i32 },
    Load { op: LoadOp, rd: Reg, rs1: Reg, offset: i32 },
    Store { op: StoreOp, rs1: Reg, rs2: Reg, offset: i32 },
    /// Register-immediate ALU op. For shifts `imm` is the shift amount (0..=31). Never `Sub`.
    OpImm { op: AluOp, rd: Reg, rs1: Reg, imm: i32 },
    Op { op: AluOp, rd: Reg, rs1: Reg, rs2: Reg },
    MulDiv { op: MulOp, rd: Reg, rs1: Reg, rs2: Reg },
    /// `src` is a register number, or the 5-bit immediate when `uimm` is true.
    Csr { op: CsrOp, rd: Reg, src: u8, uimm: bool, csr: u16 },
    Fence,
    FenceI,
    Ecall,
    Ebreak,
    Mret,
    Wfi,
    /// The raw parcel (16-bit for compressed, 32-bit otherwise); becomes `mtval`.
    Illegal(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub instr: Instr,
    pub len: u8,
    /// The instruction bits actually fetched (16-bit parcel for compressed); reported as `mtval` on faults.
    pub raw: u32,
}
```

- [ ] **Step 2: Écrire les tests du décodeur 32 bits**

En bas de `decode.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::instr::*;
    fn d(raw: u32) -> Instr { let x = decode(raw); assert_eq!(x.len, 4); x.instr }

    #[test]
    fn decodes_base_integer_instructions() {
        assert_eq!(d(0x0050_0093), Instr::OpImm { op: AluOp::Add, rd: 1, rs1: 0, imm: 5 });
        assert_eq!(d(0x0020_81b3), Instr::Op { op: AluOp::Add, rd: 3, rs1: 1, rs2: 2 });
        assert_eq!(d(0x4020_81b3), Instr::Op { op: AluOp::Sub, rd: 3, rs1: 1, rs2: 2 });
        assert_eq!(d(0x1234_52b7), Instr::Lui { rd: 5, imm: 0x1234_5000 });
        assert_eq!(d(0xffdf_f0ef), Instr::Jal { rd: 1, offset: -4 });
        assert_eq!(d(0x0020_8463), Instr::Branch { op: BranchOp::Eq, rs1: 1, rs2: 2, offset: 8 });
        assert_eq!(d(0x0081_2283), Instr::Load { op: LoadOp::W, rd: 5, rs1: 2, offset: 8 });
        assert_eq!(d(0x0051_2623), Instr::Store { op: StoreOp::W, rs1: 2, rs2: 5, offset: 12 });
        assert_eq!(d(0x4030_d093), Instr::OpImm { op: AluOp::Sra, rd: 1, rs1: 1, imm: 3 });
    }

    #[test]
    fn decodes_m_extension() {
        assert_eq!(d(0x0220_81b3), Instr::MulDiv { op: MulOp::Mul, rd: 3, rs1: 1, rs2: 2 });
        assert_eq!(d(0x0220_d1b3), Instr::MulDiv { op: MulOp::Divu, rd: 3, rs1: 1, rs2: 2 });
    }

    #[test]
    fn decodes_system_instructions() {
        assert_eq!(d(0x0000_0073), Instr::Ecall);
        assert_eq!(d(0x0010_0073), Instr::Ebreak);
        assert_eq!(d(0x3020_0073), Instr::Mret);
        assert_eq!(d(0x1050_0073), Instr::Wfi);
        assert_eq!(d(0x3001_10f3), Instr::Csr { op: CsrOp::Rw, rd: 1, src: 2, uimm: false, csr: 0x300 });
        assert_eq!(d(0x0000_000f), Instr::Fence);
        assert_eq!(d(0x0000_100f), Instr::FenceI);
    }

    #[test]
    fn illegal_encodings_trap() {
        for raw in [0xFFFF_FFFF, 0x0200_9093 /* slli with shamt[5]=1: reserved on RV32 */,
                    0x0000_2063 /* branch funct3=2 */, 0x0000_3003 /* load funct3=3 */,
                    0x0000_4073 /* system funct3=4 */, 0x0000_007F /* reserved opcode */] {
            assert_eq!(d(raw), Instr::Illegal(raw), "{raw:#010x}");
        }
    }
}
```

- [ ] **Step 3: Écrire les tests du décodeur compressé**

En bas de `decode_c.rs` :
```rust
#[cfg(test)]
mod tests {
    use crate::decode::decode;
    use crate::instr::*;
    fn c(raw: u16) -> Instr { let x = decode(u32::from(raw)); assert_eq!(x.len, 2); x.instr }

    #[test]
    fn expands_compressed_instructions() {
        assert_eq!(c(0x0085), Instr::OpImm { op: AluOp::Add, rd: 1, rs1: 1, imm: 1 });          // c.addi ra,1
        assert_eq!(c(0x557d), Instr::OpImm { op: AluOp::Add, rd: 10, rs1: 0, imm: -1 });        // c.li a0,-1
        assert_eq!(c(0x852e), Instr::Op { op: AluOp::Add, rd: 10, rs1: 0, rs2: 11 });           // c.mv a0,a1
        assert_eq!(c(0x8082), Instr::Jalr { rd: 0, rs1: 1, offset: 0 });                        // c.jr ra (ret)
        assert_eq!(c(0x4092), Instr::Load { op: LoadOp::W, rd: 1, rs1: 2, offset: 4 });         // c.lwsp ra,4(sp)
        assert_eq!(c(0xc206), Instr::Store { op: StoreOp::W, rs1: 2, rs2: 1, offset: 4 });      // c.swsp ra,4(sp)
        assert_eq!(c(0x0800), Instr::OpImm { op: AluOp::Add, rd: 8, rs1: 2, imm: 16 });         // c.addi4spn s0,sp,16
        assert_eq!(c(0xc001), Instr::Branch { op: BranchOp::Eq, rs1: 8, rs2: 0, offset: 0 });   // c.beqz s0,0
        assert_eq!(c(0xbffd), Instr::Jal { rd: 0, offset: -2 });                                // c.j -2
        assert_eq!(c(0x9002), Instr::Ebreak);                                                   // c.ebreak
    }

    #[test]
    fn illegal_compressed_encodings_trap() {
        for raw in [0x0000u16 /* all-zero parcel */, 0x0004 /* c.addi4spn with zero immediate */,
                    0x9001 /* c.srli with shamt[5]=1: reserved on RV32 */, 0x2000 /* c.fld: no F extension */,
                    0x4002 /* c.lwsp with rd=0 */, 0x8002 /* c.jr with rs1=0 */] {
            assert_eq!(c(raw), Instr::Illegal(u32::from(raw)), "{raw:#06x}");
        }
    }
}
```

- [ ] **Step 4: Constater l'échec**

Run: `cargo test -p cpu-rv32`
Expected: erreurs de compilation (`decode` inexistant).

- [ ] **Step 5: Implémenter le décodeur 32 bits**

`decode.rs` :
```rust
//! RV32IM + Zicsr decoder (32-bit encodings); compressed parcels are delegated to `decode_c`.
use crate::decode_c::decode_compressed;
use crate::instr::{AluOp, BranchOp, CsrOp, Decoded, Instr, LoadOp, MulOp, Reg, StoreOp};

const OP_LUI: u32 = 0x37;
const OP_AUIPC: u32 = 0x17;
const OP_JAL: u32 = 0x6f;
const OP_JALR: u32 = 0x67;
const OP_BRANCH: u32 = 0x63;
const OP_LOAD: u32 = 0x03;
const OP_STORE: u32 = 0x23;
const OP_IMM: u32 = 0x13;
const OP_REG: u32 = 0x33;
const OP_MISC_MEM: u32 = 0x0f;
const OP_SYSTEM: u32 = 0x73;

const FUNCT7_BASE: u32 = 0x00;
const FUNCT7_ALT: u32 = 0x20;
const FUNCT7_MULDIV: u32 = 0x01;

const ENC_ECALL: u32 = 0x0000_0073;
const ENC_EBREAK: u32 = 0x0010_0073;
const ENC_MRET: u32 = 0x3020_0073;
const ENC_WFI: u32 = 0x1050_0073;

const COMPRESSED_MASK: u32 = 0b11;

#[must_use]
pub fn decode(raw: u32) -> Decoded {
    if raw & COMPRESSED_MASK != COMPRESSED_MASK {
        // INVARIANT: the mask keeps exactly 16 bits.
        #[allow(clippy::cast_possible_truncation)]
        let parcel = (raw & 0xffff) as u16;
        return Decoded { instr: decode_compressed(parcel), len: 2, raw: u32::from(parcel) };
    }
    Decoded { instr: decode32(raw), len: 4, raw }
}

#[allow(clippy::cast_possible_truncation)] // every register field is masked to 5 bits
fn reg(raw: u32, shift: u32) -> Reg { ((raw >> shift) & 0x1f) as Reg }

#[allow(clippy::cast_possible_wrap)]
fn imm_i(raw: u32) -> i32 { (raw as i32) >> 20 }
#[allow(clippy::cast_possible_wrap)]
fn imm_s(raw: u32) -> i32 { ((raw as i32) >> 25 << 5) | ((raw >> 7) & 0x1f) as i32 }
#[allow(clippy::cast_possible_wrap)]
fn imm_b(raw: u32) -> i32 {
    ((raw as i32) >> 31 << 12) | (((raw >> 7) & 1) << 11) as i32 | (((raw >> 25) & 0x3f) << 5) as i32 | (((raw >> 8) & 0xf) << 1) as i32
}
#[allow(clippy::cast_possible_wrap)]
fn imm_j(raw: u32) -> i32 {
    ((raw as i32) >> 31 << 20) | (raw & 0x000f_f000) as i32 | (((raw >> 20) & 1) << 11) as i32 | (((raw >> 21) & 0x3ff) << 1) as i32
}

fn decode32(raw: u32) -> Instr {
    let (rd, rs1, rs2) = (reg(raw, 7), reg(raw, 15), reg(raw, 20));
    let funct3 = (raw >> 12) & 0x7;
    let funct7 = raw >> 25;
    let illegal = Instr::Illegal(raw);
    match raw & 0x7f {
        OP_LUI => Instr::Lui { rd, imm: raw & 0xffff_f000 },
        OP_AUIPC => Instr::Auipc { rd, imm: raw & 0xffff_f000 },
        OP_JAL => Instr::Jal { rd, offset: imm_j(raw) },
        OP_JALR if funct3 == 0 => Instr::Jalr { rd, rs1, offset: imm_i(raw) },
        OP_BRANCH => match funct3 {
            0 => Instr::Branch { op: BranchOp::Eq, rs1, rs2, offset: imm_b(raw) },
            1 => Instr::Branch { op: BranchOp::Ne, rs1, rs2, offset: imm_b(raw) },
            4 => Instr::Branch { op: BranchOp::Lt, rs1, rs2, offset: imm_b(raw) },
            5 => Instr::Branch { op: BranchOp::Ge, rs1, rs2, offset: imm_b(raw) },
            6 => Instr::Branch { op: BranchOp::Ltu, rs1, rs2, offset: imm_b(raw) },
            7 => Instr::Branch { op: BranchOp::Geu, rs1, rs2, offset: imm_b(raw) },
            _ => illegal,
        },
        OP_LOAD => {
            let op = match funct3 { 0 => LoadOp::B, 1 => LoadOp::H, 2 => LoadOp::W, 4 => LoadOp::Bu, 5 => LoadOp::Hu, _ => return illegal };
            Instr::Load { op, rd, rs1, offset: imm_i(raw) }
        }
        OP_STORE => {
            let op = match funct3 { 0 => StoreOp::B, 1 => StoreOp::H, 2 => StoreOp::W, _ => return illegal };
            Instr::Store { op, rs1, rs2, offset: imm_s(raw) }
        }
        OP_IMM => decode_op_imm(raw, rd, rs1, funct3, funct7),
        OP_REG => decode_op_reg(raw, rd, rs1, rs2, funct3, funct7),
        OP_MISC_MEM => match funct3 { 0 => Instr::Fence, 1 => Instr::FenceI, _ => illegal },
        OP_SYSTEM => decode_system(raw, rd, rs1, funct3),
        _ => illegal,
    }
}

fn decode_op_imm(raw: u32, rd: Reg, rs1: Reg, funct3: u32, funct7: u32) -> Instr {
    let shamt = i32::from(reg(raw, 20));
    let op = match (funct3, funct7) {
        (0, _) => AluOp::Add,
        (2, _) => AluOp::Slt,
        (3, _) => AluOp::Sltu,
        (4, _) => AluOp::Xor,
        (6, _) => AluOp::Or,
        (7, _) => AluOp::And,
        (1, FUNCT7_BASE) => return Instr::OpImm { op: AluOp::Sll, rd, rs1, imm: shamt },
        (5, FUNCT7_BASE) => return Instr::OpImm { op: AluOp::Srl, rd, rs1, imm: shamt },
        (5, FUNCT7_ALT) => return Instr::OpImm { op: AluOp::Sra, rd, rs1, imm: shamt },
        _ => return Instr::Illegal(raw),
    };
    Instr::OpImm { op, rd, rs1, imm: imm_i(raw) }
}

fn decode_op_reg(raw: u32, rd: Reg, rs1: Reg, rs2: Reg, funct3: u32, funct7: u32) -> Instr {
    if funct7 == FUNCT7_MULDIV {
        let op = [MulOp::Mul, MulOp::Mulh, MulOp::Mulhsu, MulOp::Mulhu, MulOp::Div, MulOp::Divu, MulOp::Rem, MulOp::Remu][funct3 as usize];
        return Instr::MulDiv { op, rd, rs1, rs2 };
    }
    let op = match (funct3, funct7) {
        (0, FUNCT7_BASE) => AluOp::Add,
        (0, FUNCT7_ALT) => AluOp::Sub,
        (1, FUNCT7_BASE) => AluOp::Sll,
        (2, FUNCT7_BASE) => AluOp::Slt,
        (3, FUNCT7_BASE) => AluOp::Sltu,
        (4, FUNCT7_BASE) => AluOp::Xor,
        (5, FUNCT7_BASE) => AluOp::Srl,
        (5, FUNCT7_ALT) => AluOp::Sra,
        (6, FUNCT7_BASE) => AluOp::Or,
        (7, FUNCT7_BASE) => AluOp::And,
        _ => return Instr::Illegal(raw),
    };
    Instr::Op { op, rd, rs1, rs2 }
}

fn decode_system(raw: u32, rd: Reg, rs1: Reg, funct3: u32) -> Instr {
    // INVARIANT: the CSR address field is 12 bits.
    #[allow(clippy::cast_possible_truncation)]
    let csr = (raw >> 20) as u16;
    let csr_instr = |op, uimm| Instr::Csr { op, rd, src: rs1, uimm, csr };
    match funct3 {
        0 => match raw {
            ENC_ECALL => Instr::Ecall,
            ENC_EBREAK => Instr::Ebreak,
            ENC_MRET => Instr::Mret,
            ENC_WFI => Instr::Wfi,
            _ => Instr::Illegal(raw),
        },
        1 => csr_instr(CsrOp::Rw, false),
        2 => csr_instr(CsrOp::Rs, false),
        3 => csr_instr(CsrOp::Rc, false),
        5 => csr_instr(CsrOp::Rw, true),
        6 => csr_instr(CsrOp::Rs, true),
        7 => csr_instr(CsrOp::Rc, true),
        _ => Instr::Illegal(raw),
    }
}
```

- [ ] **Step 6: Implémenter le décodeur compressé**

`decode_c.rs` :
```rust
//! RVC (compressed) decoder for RV32: expands each 16-bit parcel to its 32-bit equivalent `Instr`.
use crate::instr::{AluOp, BranchOp, Instr, LoadOp, Reg, StoreOp};

const REG_ZERO: Reg = 0;
const REG_RA: Reg = 1;
const REG_SP: Reg = 2;
const PRIME_REG_BASE: Reg = 8;

#[allow(clippy::cast_possible_truncation)] // fields are masked before the cast
fn full_reg(h: u16, shift: u16) -> Reg { ((h >> shift) & 0x1f) as Reg }
#[allow(clippy::cast_possible_truncation)]
fn prime_reg(h: u16, shift: u16) -> Reg { ((h >> shift) & 0x7) as Reg + PRIME_REG_BASE }

/// Sign-extends the low `bits` bits of `value`.
#[allow(clippy::cast_possible_wrap)]
fn sext(value: u32, bits: u32) -> i32 { ((value << (32 - bits)) as i32) >> (32 - bits) }

/// 6-bit immediate of CI-format instructions: imm[5] = bit 12, imm[4:0] = bits 6:2.
fn ci_imm(h: u16) -> i32 { let h = u32::from(h); sext(((h >> 7) & 0x20) | ((h >> 2) & 0x1f), 6) }
/// Shift amount of c.slli/c.srli/c.srai; `None` when shamt[5] is set (reserved on RV32).
fn ci_shamt(h: u16) -> Option<i32> { if h & 0x1000 != 0 { None } else { Some(i32::from((h >> 2) & 0x1f)) } }

fn cj_offset(h: u16) -> i32 {
    let h = u32::from(h);
    let v = ((h >> 1) & 0x800) | ((h >> 7) & 0x10) | ((h >> 1) & 0x300) | ((h << 2) & 0x400)
        | ((h >> 1) & 0x40) | ((h << 1) & 0x80) | ((h >> 2) & 0xe) | ((h << 3) & 0x20);
    sext(v, 12)
}

fn cb_offset(h: u16) -> i32 {
    let h = u32::from(h);
    let v = ((h >> 4) & 0x100) | ((h >> 7) & 0x18) | ((h << 1) & 0xc0) | ((h >> 2) & 0x6) | ((h << 3) & 0x20);
    sext(v, 9)
}

#[must_use]
pub fn decode_compressed(h: u16) -> Instr {
    let illegal = Instr::Illegal(u32::from(h));
    if h == 0 { return illegal; }
    let funct3 = (h >> 13) & 0x7;
    match (h & 0x3, funct3) {
        (0, 0) => {
            let hu = u32::from(h);
            let imm = ((hu >> 7) & 0x30) | ((hu >> 1) & 0x3c0) | ((hu >> 4) & 0x4) | ((hu >> 2) & 0x8);
            if imm == 0 { return illegal; }
            #[allow(clippy::cast_possible_wrap)] // at most 10 bits
            Instr::OpImm { op: AluOp::Add, rd: prime_reg(h, 2), rs1: REG_SP, imm: imm as i32 }
        }
        (0, 2) | (0, 6) => {
            let hu = u32::from(h);
            #[allow(clippy::cast_possible_wrap)] // at most 7 bits
            let offset = (((hu >> 7) & 0x38) | ((hu >> 4) & 0x4) | ((hu << 1) & 0x40)) as i32;
            if funct3 == 2 { Instr::Load { op: LoadOp::W, rd: prime_reg(h, 2), rs1: prime_reg(h, 7), offset } }
            else { Instr::Store { op: StoreOp::W, rs1: prime_reg(h, 7), rs2: prime_reg(h, 2), offset } }
        }
        (1, 0) => { let rd = full_reg(h, 7); Instr::OpImm { op: AluOp::Add, rd, rs1: rd, imm: ci_imm(h) } }
        (1, 1) => Instr::Jal { rd: REG_RA, offset: cj_offset(h) },
        (1, 2) => Instr::OpImm { op: AluOp::Add, rd: full_reg(h, 7), rs1: REG_ZERO, imm: ci_imm(h) },
        (1, 3) => decode_lui_addi16sp(h),
        (1, 4) => decode_misc_alu(h),
        (1, 5) => Instr::Jal { rd: REG_ZERO, offset: cj_offset(h) },
        (1, 6) => Instr::Branch { op: BranchOp::Eq, rs1: prime_reg(h, 7), rs2: REG_ZERO, offset: cb_offset(h) },
        (1, 7) => Instr::Branch { op: BranchOp::Ne, rs1: prime_reg(h, 7), rs2: REG_ZERO, offset: cb_offset(h) },
        (2, 0) => match ci_shamt(h) {
            Some(imm) => { let rd = full_reg(h, 7); Instr::OpImm { op: AluOp::Sll, rd, rs1: rd, imm } }
            None => illegal,
        },
        (2, 2) => {
            let rd = full_reg(h, 7);
            if rd == REG_ZERO { return illegal; }
            let hu = u32::from(h);
            #[allow(clippy::cast_possible_wrap)] // at most 8 bits
            let offset = (((hu >> 7) & 0x20) | ((hu >> 2) & 0x1c) | ((hu << 4) & 0xc0)) as i32;
            Instr::Load { op: LoadOp::W, rd, rs1: REG_SP, offset }
        }
        (2, 4) => decode_jr_mv_add(h),
        (2, 6) => {
            let hu = u32::from(h);
            #[allow(clippy::cast_possible_wrap)] // at most 8 bits
            let offset = (((hu >> 7) & 0x3c) | ((hu >> 1) & 0xc0)) as i32;
            Instr::Store { op: StoreOp::W, rs1: REG_SP, rs2: full_reg(h, 2), offset }
        }
        _ => illegal,
    }
}

fn decode_lui_addi16sp(h: u16) -> Instr {
    let rd = full_reg(h, 7);
    let hu = u32::from(h);
    if rd == REG_SP {
        let v = ((hu >> 3) & 0x200) | ((hu >> 2) & 0x10) | ((hu << 1) & 0x40) | ((hu << 4) & 0x180) | ((hu << 3) & 0x20);
        let imm = sext(v, 10);
        if imm == 0 { return Instr::Illegal(hu); }
        return Instr::OpImm { op: AluOp::Add, rd: REG_SP, rs1: REG_SP, imm };
    }
    let imm = sext((((hu >> 12) & 1) << 17) | (((hu >> 2) & 0x1f) << 12), 18);
    if imm == 0 { return Instr::Illegal(hu); }
    #[allow(clippy::cast_sign_loss)] // LUI takes the raw upper bits
    Instr::Lui { rd, imm: imm as u32 }
}

fn decode_misc_alu(h: u16) -> Instr {
    let rd = prime_reg(h, 7);
    let illegal = Instr::Illegal(u32::from(h));
    match (h >> 10) & 0x3 {
        0 => ci_shamt(h).map_or(illegal, |imm| Instr::OpImm { op: AluOp::Srl, rd, rs1: rd, imm }),
        1 => ci_shamt(h).map_or(illegal, |imm| Instr::OpImm { op: AluOp::Sra, rd, rs1: rd, imm }),
        2 => Instr::OpImm { op: AluOp::And, rd, rs1: rd, imm: ci_imm(h) },
        _ => {
            if h & 0x1000 != 0 { return illegal; } // c.subw/c.addw: RV64 only
            let op = [AluOp::Sub, AluOp::Xor, AluOp::Or, AluOp::And][usize::from((h >> 5) & 0x3)];
            Instr::Op { op, rd, rs1: rd, rs2: prime_reg(h, 2) }
        }
    }
}

fn decode_jr_mv_add(h: u16) -> Instr {
    let (rs1, rs2) = (full_reg(h, 7), full_reg(h, 2));
    let bit12 = h & 0x1000 != 0;
    match (bit12, rs1, rs2) {
        (false, REG_ZERO, REG_ZERO) => Instr::Illegal(u32::from(h)),
        (false, _, REG_ZERO) => Instr::Jalr { rd: REG_ZERO, rs1, offset: 0 },
        (false, _, _) => Instr::Op { op: AluOp::Add, rd: rs1, rs1: REG_ZERO, rs2 },
        (true, REG_ZERO, REG_ZERO) => Instr::Ebreak,
        (true, _, REG_ZERO) => Instr::Jalr { rd: REG_RA, rs1, offset: 0 },
        (true, _, _) => Instr::Op { op: AluOp::Add, rd: rs1, rs1, rs2 },
    }
}
```
Dans `lib.rs` : `pub mod instr; pub mod decode; mod decode_c;` + `pub use decode::decode; pub use instr::*;` et `#![cfg_attr(test, allow(clippy::unwrap_used))]`.

- [ ] **Step 7: Lancer les tests**

Run: `cargo test -p cpu-rv32 && cargo clippy -p cpu-rv32 --all-targets -- -D warnings`
Expected: PASS. Si un encodage de test échoue, vérifier d'abord l'encodage avec `riscv64-unknown-elf-objdump` (image Docker de la Task 7) avant de toucher au décodeur : le test est la spécification, mais il peut contenir une erreur de saisie.

- [ ] **Step 8: Commit**

```bash
git commit -m "feat(rv32): RV32IMC + Zicsr decoder with compressed expansion" -- crates/cpu-rv32
```

---

### Task 5: `cpu-rv32` — hart, CSR machine, exceptions, interruptions, exécution

Référence : *RISC-V Privileged Architecture* (mode machine seul : M-mode, `mstatus`, `mtvec` direct/vectorisé, `mcause`, `mtval`, `mret`, `wfi`). L'ESP32-C3 n'a que le mode machine (+ U-mode non utilisé par ESP-IDF) : U-mode est hors périmètre et `mstatus.MPP` reste figé à M.

Les accès non alignés sont une **politique** (`MisalignedPolicy`), car le comportement exact du C3 sera arbitré par le lockstep QEMU en SP1b. La valeur par défaut est `Trap`.

Avec l'extension C, une cible de saut est toujours paire (JAL/branch encodent des offsets pairs, JALR efface le bit 0). L'exception « saut non aligné » (cause 0) ne peut donc pas se produire sur ce cœur : il n'y a rien à tester pour elle.

**Files:**
- Create: `crates/cpu-rv32/src/{trap.rs,csr.rs,hart.rs,exec.rs,asm.rs}`
- Modify: `crates/cpu-rv32/src/lib.rs`

**Interfaces:**
- Consumes: `emu_core::{Bus, AccessSize, AccessKind, BusError, Cycles, InstrClass, TimingModel}`, `crate::{decode, Instr, ...}`.
- Produces :
  - `enum Exception { InstrAccessFault, IllegalInstr, Breakpoint, LoadMisaligned, LoadAccessFault, StoreMisaligned, StoreAccessFault, EcallM }` avec `fn code(self) -> u32` (1, 2, 3, 4, 5, 6, 7, 11).
  - `struct Trap { cause: Exception, tval: u32 }`.
  - `enum MisalignedPolicy { Trap, Allow }`, `struct HartConfig { misaligned: MisalignedPolicy }` (`Default` → `Trap`).
  - `struct Hart` : champs publics `x: [u32; 32]`, `pc: u32`, `csr: CsrFile`, `cycles: Cycles`, `instret: u64`.
  - `Hart::new(reset_pc: u32, config: HartConfig) -> Hart`.
  - `fn step(&mut self, bus: &mut dyn Bus, timing: &dyn TimingModel) -> StepOutcome`.
  - `enum StepOutcome { Retired { cycles: u32 }, Trapped { trap: Trap, cycles: u32 }, Interrupted { cause: u32 }, Waiting }`.
  - `fn set_interrupt_pending(&mut self, line: u32, pending: bool)` (lignes 0..=31, bits de `mip`).
  - `CsrFile` : `fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32>`, `fn write(&mut self, addr: u16, value: u32) -> bool` (false = CSR inconnu ou en lecture seule), constantes `MSTATUS, MISA, MIE, MTVEC, MSCRATCH, MEPC, MCAUSE, MTVAL, MIP, MCYCLE, MCYCLEH, MINSTRET, MINSTRETH, CYCLE, CYCLEH, INSTRET, INSTRETH, MVENDORID, MARCHID, MIMPID, MHARTID`.
  - `#[cfg(test)] pub(crate) mod asm` : encodeurs `addi, add, sub, lui, auipc, jal, jalr, beq, bne, blt, lw, lh, lb, lbu, sw, sb, mul, mulh, mulhsu, mulhu, div, divu, rem, remu, csrrw, csrrs, csrrwi, ecall, ebreak, mret, wfi` (signatures `fn addi(rd: u8, rs1: u8, imm: i32) -> u32`, etc.).

- [ ] **Step 1: Mini-assembleur de test**

`asm.rs` (compilé uniquement en test) :
```rust
//! Test-only RV32 encoder, so tests read as assembly instead of hex.
#![allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]

fn r(f7: u32, rs2: u8, rs1: u8, f3: u32, rd: u8, op: u32) -> u32 {
    (f7 << 25) | (u32::from(rs2) << 20) | (u32::from(rs1) << 15) | (f3 << 12) | (u32::from(rd) << 7) | op
}
fn i(imm: i32, rs1: u8, f3: u32, rd: u8, op: u32) -> u32 {
    ((imm as u32 & 0xfff) << 20) | (u32::from(rs1) << 15) | (f3 << 12) | (u32::from(rd) << 7) | op
}
fn s(imm: i32, rs2: u8, rs1: u8, f3: u32, op: u32) -> u32 {
    let imm = imm as u32;
    (((imm >> 5) & 0x7f) << 25) | (u32::from(rs2) << 20) | (u32::from(rs1) << 15) | (f3 << 12) | ((imm & 0x1f) << 7) | op
}
fn b(imm: i32, rs2: u8, rs1: u8, f3: u32) -> u32 {
    let imm = imm as u32;
    (((imm >> 12) & 1) << 31) | (((imm >> 5) & 0x3f) << 25) | (u32::from(rs2) << 20) | (u32::from(rs1) << 15)
        | (f3 << 12) | (((imm >> 1) & 0xf) << 8) | (((imm >> 11) & 1) << 7) | 0x63
}
pub fn addi(rd: u8, rs1: u8, imm: i32) -> u32 { i(imm, rs1, 0, rd, 0x13) }
pub fn add(rd: u8, rs1: u8, rs2: u8) -> u32 { r(0, rs2, rs1, 0, rd, 0x33) }
pub fn sub(rd: u8, rs1: u8, rs2: u8) -> u32 { r(0x20, rs2, rs1, 0, rd, 0x33) }
pub fn lui(rd: u8, imm: u32) -> u32 { (imm & 0xffff_f000) | (u32::from(rd) << 7) | 0x37 }
pub fn auipc(rd: u8, imm: u32) -> u32 { (imm & 0xffff_f000) | (u32::from(rd) << 7) | 0x17 }
pub fn jal(rd: u8, imm: i32) -> u32 {
    let imm = imm as u32;
    (((imm >> 20) & 1) << 31) | (((imm >> 1) & 0x3ff) << 21) | (((imm >> 11) & 1) << 20) | (imm & 0xff000) | (u32::from(rd) << 7) | 0x6f
}
pub fn jalr(rd: u8, rs1: u8, imm: i32) -> u32 { i(imm, rs1, 0, rd, 0x67) }
pub fn beq(rs1: u8, rs2: u8, imm: i32) -> u32 { b(imm, rs2, rs1, 0) }
pub fn bne(rs1: u8, rs2: u8, imm: i32) -> u32 { b(imm, rs2, rs1, 1) }
pub fn blt(rs1: u8, rs2: u8, imm: i32) -> u32 { b(imm, rs2, rs1, 4) }
pub fn lb(rd: u8, rs1: u8, imm: i32) -> u32 { i(imm, rs1, 0, rd, 0x03) }
pub fn lh(rd: u8, rs1: u8, imm: i32) -> u32 { i(imm, rs1, 1, rd, 0x03) }
pub fn lw(rd: u8, rs1: u8, imm: i32) -> u32 { i(imm, rs1, 2, rd, 0x03) }
pub fn lbu(rd: u8, rs1: u8, imm: i32) -> u32 { i(imm, rs1, 4, rd, 0x03) }
pub fn sb(rs2: u8, rs1: u8, imm: i32) -> u32 { s(imm, rs2, rs1, 0, 0x23) }
pub fn sw(rs2: u8, rs1: u8, imm: i32) -> u32 { s(imm, rs2, rs1, 2, 0x23) }
pub fn mul(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 0, rd, 0x33) }
pub fn mulh(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 1, rd, 0x33) }
pub fn mulhsu(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 2, rd, 0x33) }
pub fn mulhu(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 3, rd, 0x33) }
pub fn div(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 4, rd, 0x33) }
pub fn divu(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 5, rd, 0x33) }
pub fn rem(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 6, rd, 0x33) }
pub fn remu(rd: u8, rs1: u8, rs2: u8) -> u32 { r(1, rs2, rs1, 7, rd, 0x33) }
pub fn csrrw(rd: u8, csr: u16, rs1: u8) -> u32 { (u32::from(csr) << 20) | (u32::from(rs1) << 15) | (1 << 12) | (u32::from(rd) << 7) | 0x73 }
pub fn csrrs(rd: u8, csr: u16, rs1: u8) -> u32 { (u32::from(csr) << 20) | (u32::from(rs1) << 15) | (2 << 12) | (u32::from(rd) << 7) | 0x73 }
pub fn csrrwi(rd: u8, csr: u16, uimm: u8) -> u32 { (u32::from(csr) << 20) | (u32::from(uimm) << 15) | (5 << 12) | (u32::from(rd) << 7) | 0x73 }
pub const fn ecall() -> u32 { 0x0000_0073 }
pub const fn ebreak() -> u32 { 0x0010_0073 }
pub const fn mret() -> u32 { 0x3020_0073 }
pub const fn wfi() -> u32 { 0x1050_0073 }
```
Premier test, qui vérifie l'assembleur contre les encodages déjà validés en Task 4 :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoder_matches_reference_encodings() {
        assert_eq!(addi(1, 0, 5), 0x0050_0093);
        assert_eq!(sub(3, 1, 2), 0x4020_81b3);
        assert_eq!(jal(1, -4), 0xffdf_f0ef);
        assert_eq!(beq(1, 2, 8), 0x0020_8463);
        assert_eq!(sw(5, 2, 12), 0x0051_2623);
        assert_eq!(csrrw(1, 0x300, 2), 0x3001_10f3);
    }
}
```

- [ ] **Step 2: Écrire les tests du hart (échouent)**

En bas de `hart.rs`. Le banc de test : RAM de 64 Kio à `0x0`, programme à `0x0`, `mtvec` = `0x1000`, et un modèle de timing trivial (1 cycle par instruction).
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::*;
    use crate::csr::*;
    use emu_core::{AccessKind, AccessSize, Bus, InstrClass, Ram, TimingModel};

    #[derive(Debug)]
    struct OneCycle;
    impl TimingModel for OneCycle {
        fn name(&self) -> &'static str { "one-cycle" }
        fn is_calibrated(&self) -> bool { false }
        fn instr_cycles(&self, _: InstrClass) -> u32 { 1 }
    }

    const TRAP_VECTOR: u32 = 0x1000;

    fn machine(program: &[u32], policy: MisalignedPolicy) -> (Hart, Ram) {
        let mut ram = Ram::new(0, 64 * 1024);
        let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
        ram.load(0, &bytes).unwrap();
        let mut hart = Hart::new(0, HartConfig { misaligned: policy });
        assert!(hart.csr.write(MTVEC, TRAP_VECTOR));
        (hart, ram)
    }

    fn run(hart: &mut Hart, ram: &mut Ram, steps: usize) -> Vec<StepOutcome> {
        (0..steps).map(|_| hart.step(ram, &OneCycle)).collect()
    }

    #[test]
    fn arithmetic_and_x0_hardwired() {
        let (mut h, mut m) = machine(&[addi(1, 0, 7), addi(2, 0, -3), add(3, 1, 2), sub(4, 2, 1), addi(0, 0, 9)], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 5);
        assert_eq!((h.x[3], h.x[4] as i32, h.x[0]), (4, -10, 0));
        assert_eq!((h.pc, h.instret), (20, 5));
    }

    #[test]
    fn lui_auipc_and_jumps_link() {
        let (mut h, mut m) = machine(&[lui(5, 0x1234_5000), auipc(6, 0x1000), jal(1, 8), addi(7, 0, 1), jalr(2, 1, 0)], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 4);
        assert_eq!(h.x[5], 0x1234_5000);
        assert_eq!(h.x[6], 0x1004);          // auipc at pc=4
        assert_eq!(h.x[1], 12);              // jal at pc=8 links to 12, jumps to 16
        assert_eq!(h.x[7], 0);               // skipped
        assert_eq!((h.x[2], h.pc), (20, 12)); // jalr at 16 returns to x1=12
    }

    #[test]
    fn branches_report_taken_and_not_taken() {
        let (mut h, mut m) = machine(&[addi(1, 0, 1), beq(1, 0, 8), bne(1, 0, 8), addi(2, 0, 1), blt(0, 1, -4)], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 3);
        assert_eq!(h.pc, 16);                // beq not taken (4→8), bne taken (8→16)
        assert_eq!(h.x[2], 0);
    }

    #[test]
    fn loads_sign_and_zero_extend() {
        let (mut h, mut m) = machine(&[addi(1, 0, 0x200), addi(2, 0, -1), sb(2, 1, 0), lb(3, 1, 0), lbu(4, 1, 0), lh(5, 1, 0), sw(2, 1, 4), lw(6, 1, 4)], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 8);
        assert_eq!((h.x[3], h.x[4], h.x[5], h.x[6]), (0xffff_ffff, 0xff, 0x0000_00ff, 0xffff_ffff));
    }

    #[test]
    fn mul_div_edge_cases_follow_the_spec() {
        let (mut h, mut m) = machine(&[
            addi(1, 0, -1), lui(2, 0x8000_0000), addi(3, 0, 0),
            mulh(10, 1, 1), mulhu(11, 1, 1), mulhsu(12, 1, 1), mul(13, 1, 1),
            div(14, 1, 3), divu(15, 1, 3), rem(16, 1, 3), remu(17, 2, 3),
            div(18, 2, 1), rem(19, 2, 1),
        ], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 13);
        assert_eq!(h.x[10], 0);              // (-1)*(-1) = 1, high word 0
        assert_eq!(h.x[11], 0xffff_fffe);    // 0xffffffff^2 high word
        assert_eq!(h.x[12], 0xffff_ffff);    // -1 * 0xffffffff, high word
        assert_eq!(h.x[13], 1);
        assert_eq!((h.x[14], h.x[15]), (0xffff_ffff, 0xffff_ffff)); // division by zero
        assert_eq!((h.x[16], h.x[17]), (0xffff_ffff, 0x8000_0000)); // remainder by zero = dividend
        assert_eq!((h.x[18], h.x[19]), (0x8000_0000, 0));           // INT_MIN / -1 overflow
    }

    #[test]
    fn unmapped_load_traps() {
        let (mut h, mut m) = machine(&[lui(1, 0x8000_0000), lw(2, 1, 0)], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 2);
        assert_eq!(out[1], StepOutcome::Trapped { trap: Trap { cause: Exception::LoadAccessFault, tval: 0x8000_0000 }, cycles: 1 });
        assert_eq!(h.pc, TRAP_VECTOR);
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(4));
        assert_eq!(h.csr.read(MCAUSE, h.cycles, h.instret), Some(5));
        assert_eq!(h.csr.read(MTVAL, h.cycles, h.instret), Some(0x8000_0000));
        assert_eq!(h.instret, 1);            // the faulting load does not retire
    }

    #[test]
    fn misaligned_load_traps_or_succeeds_by_policy() {
        let prog = [addi(1, 0, 0x201), lw(2, 1, 0)];
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 2);
        assert_eq!(out[1], StepOutcome::Trapped { trap: Trap { cause: Exception::LoadMisaligned, tval: 0x201 }, cycles: 1 });
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Allow);
        let out = run(&mut h, &mut m, 2);
        assert!(matches!(out[1], StepOutcome::Retired { .. }));
    }

    #[test]
    fn illegal_instruction_reports_raw_word() {
        let (mut h, mut m) = machine(&[0xffff_ffff], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 1);
        assert_eq!(out[0], StepOutcome::Trapped { trap: Trap { cause: Exception::IllegalInstr, tval: 0xffff_ffff }, cycles: 1 });
    }

    #[test]
    fn ecall_traps_and_mret_returns() {
        let mut prog = vec![ecall(), addi(5, 0, 1)];
        prog.resize((TRAP_VECTOR / 4) as usize, 0x0000_0013); // nops
        prog.extend([csrrs(6, MEPC, 0), addi(6, 6, 4), csrrw(0, MEPC, 6), mret()]);
        let (mut h, mut m) = machine(&prog, MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 6);
        assert!(matches!(out[0], StepOutcome::Trapped { trap: Trap { cause: Exception::EcallM, tval: 0 }, .. }));
        assert_eq!(h.x[5], 1);               // resumed after the ecall
    }

    #[test]
    fn read_only_csr_write_is_illegal_and_unknown_csr_is_illegal() {
        let (mut h, mut m) = machine(&[csrrw(1, MHARTID, 2), csrrs(1, 0x7ff, 0), csrrs(3, MHARTID, 0)], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 1);
        assert!(matches!(out[0], StepOutcome::Trapped { trap: Trap { cause: Exception::IllegalInstr, .. }, .. }));
        h.pc = 4;
        let out = run(&mut h, &mut m, 2);
        assert!(matches!(out[0], StepOutcome::Trapped { trap: Trap { cause: Exception::IllegalInstr, .. }, .. }));
        h.pc = 8;
        let out = run(&mut h, &mut m, 1);
        assert!(matches!(out[0], StepOutcome::Retired { .. }));   // csrrs x3, mhartid, x0 does not write
        assert_eq!(h.x[3], 0);
    }

    #[test]
    fn wfi_waits_then_vectored_interrupt_is_taken() {
        const LINE: u32 = 7;
        let (mut h, mut m) = machine(&[csrrwi(0, MIE, 0), wfi(), addi(1, 0, 1)], MisalignedPolicy::Trap);
        assert!(h.csr.write(MTVEC, TRAP_VECTOR | 1));           // vectored mode
        assert!(h.csr.write(MIE, 1 << LINE));
        assert!(h.csr.write(MSTATUS, MSTATUS_MIE));
        h.pc = 4;                                               // skip the mie reset
        assert!(matches!(h.step(&mut m, &OneCycle), StepOutcome::Retired { .. })); // wfi retires
        assert_eq!(h.step(&mut m, &OneCycle), StepOutcome::Waiting);
        h.set_interrupt_pending(LINE, true);
        assert_eq!(h.step(&mut m, &OneCycle), StepOutcome::Interrupted { cause: LINE });
        assert_eq!(h.pc, TRAP_VECTOR + 4 * LINE);
        assert_eq!(h.csr.read(MCAUSE, h.cycles, h.instret), Some(0x8000_0000 | LINE));
        assert_eq!(h.csr.read(MEPC, h.cycles, h.instret), Some(8));
        assert_eq!(h.csr.read(MSTATUS, h.cycles, h.instret).map(|s| s & MSTATUS_MIE), Some(0));
    }

    #[test]
    fn counters_track_cycles_and_instret() {
        let (mut h, mut m) = machine(&[addi(1, 0, 1), csrrs(2, MINSTRET, 0), csrrs(3, MCYCLE, 0)], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 3);
        assert_eq!((h.x[2], h.x[3]), (1, 2));
    }
}
```
`unused_imports` sur `AccessKind/AccessSize/Bus` : retirer de l'import celles qui ne servent pas après implémentation.

- [ ] **Step 3: Constater l'échec**

Run: `cargo test -p cpu-rv32`
Expected: erreurs de compilation.

- [ ] **Step 4: Implémenter `trap.rs`**

```rust
//! Synchronous exceptions of a machine-mode-only RV32 hart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exception { InstrAccessFault, IllegalInstr, Breakpoint, LoadMisaligned, LoadAccessFault, StoreMisaligned, StoreAccessFault, EcallM }

impl Exception {
    /// `mcause` exception code (Privileged spec, table "Machine cause register values").
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Exception::InstrAccessFault => 1, Exception::IllegalInstr => 2, Exception::Breakpoint => 3,
            Exception::LoadMisaligned => 4, Exception::LoadAccessFault => 5, Exception::StoreMisaligned => 6,
            Exception::StoreAccessFault => 7, Exception::EcallM => 11,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trap { pub cause: Exception, pub tval: u32 }
```

- [ ] **Step 5: Implémenter `csr.rs`**

```rust
//! Machine-mode CSR file (Zicsr). SoC-specific CSRs are added by the SoC crate in SP1b.
use emu_core::Cycles;

pub const MSTATUS: u16 = 0x300; pub const MISA: u16 = 0x301; pub const MIE: u16 = 0x304; pub const MTVEC: u16 = 0x305;
pub const MSCRATCH: u16 = 0x340; pub const MEPC: u16 = 0x341; pub const MCAUSE: u16 = 0x342; pub const MTVAL: u16 = 0x343; pub const MIP: u16 = 0x344;
pub const MCYCLE: u16 = 0xB00; pub const MINSTRET: u16 = 0xB02; pub const MCYCLEH: u16 = 0xB80; pub const MINSTRETH: u16 = 0xB82;
pub const CYCLE: u16 = 0xC00; pub const INSTRET: u16 = 0xC02; pub const CYCLEH: u16 = 0xC80; pub const INSTRETH: u16 = 0xC82;
pub const MVENDORID: u16 = 0xF11; pub const MARCHID: u16 = 0xF12; pub const MIMPID: u16 = 0xF13; pub const MHARTID: u16 = 0xF14;

pub const MSTATUS_MIE: u32 = 1 << 3;
pub const MSTATUS_MPIE: u32 = 1 << 7;
/// MPP is hard-wired to machine mode (0b11): this hart has no lower privilege mode in scope.
pub const MSTATUS_MPP_M: u32 = 0b11 << 11;
const MSTATUS_WRITABLE: u32 = MSTATUS_MIE | MSTATUS_MPIE;
/// MXL=1 (32-bit) | C | I | M.
const MISA_RV32IMC: u32 = (1 << 30) | (1 << 2) | (1 << 8) | (1 << 12);
/// CSR addresses whose bits [11:10] are 0b11 are read-only (Privileged spec §2.1).
const READ_ONLY_MASK: u16 = 0xC00;

#[derive(Clone, Debug, Default)]
pub struct CsrFile { pub mstatus: u32, pub mie: u32, pub mip: u32, pub mtvec: u32, pub mscratch: u32, pub mepc: u32, pub mcause: u32, pub mtval: u32, cycle_offset: u64, instret_offset: u64 }

#[allow(clippy::cast_possible_truncation)]
const fn low(v: u64) -> u32 { v as u32 }
const fn high(v: u64) -> u32 { low(v >> 32) }

impl CsrFile {
    /// Reads a CSR; `None` means "no such CSR" (illegal instruction).
    #[must_use]
    pub fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32> {
        let cyc = cycles.get().wrapping_add(self.cycle_offset);
        let ret = instret.wrapping_add(self.instret_offset);
        Some(match addr {
            MSTATUS => self.mstatus | MSTATUS_MPP_M,
            MISA => MISA_RV32IMC,
            MIE => self.mie, MIP => self.mip, MTVEC => self.mtvec, MSCRATCH => self.mscratch,
            MEPC => self.mepc, MCAUSE => self.mcause, MTVAL => self.mtval,
            MCYCLE | CYCLE => low(cyc), MCYCLEH | CYCLEH => high(cyc),
            MINSTRET | INSTRET => low(ret), MINSTRETH | INSTRETH => high(ret),
            MVENDORID | MARCHID | MIMPID | MHARTID => 0,
            _ => return None,
        })
    }

    /// Writes a CSR; returns `false` for unknown or read-only CSRs.
    /// Counter writes are stored as offsets so the hart's own counters stay monotonic.
    pub fn write_with_counters(&mut self, addr: u16, value: u32, cycles: Cycles, instret: u64) -> bool {
        if addr & READ_ONLY_MASK == READ_ONLY_MASK { return false; }
        match addr {
            MSTATUS => self.mstatus = value & MSTATUS_WRITABLE,
            MIE => self.mie = value,
            MIP => {} // pending bits are driven by the interrupt controller, not by software
            MTVEC => self.mtvec = value & !0b10, // modes 0 (direct) and 1 (vectored) only
            MSCRATCH => self.mscratch = value,
            MEPC => self.mepc = value & !1,
            MCAUSE => self.mcause = value,
            MTVAL => self.mtval = value,
            MCYCLE => self.cycle_offset = replace_low(cycles.get().wrapping_add(self.cycle_offset), value).wrapping_sub(cycles.get()),
            MCYCLEH => self.cycle_offset = replace_high(cycles.get().wrapping_add(self.cycle_offset), value).wrapping_sub(cycles.get()),
            MINSTRET => self.instret_offset = replace_low(instret.wrapping_add(self.instret_offset), value).wrapping_sub(instret),
            MINSTRETH => self.instret_offset = replace_high(instret.wrapping_add(self.instret_offset), value).wrapping_sub(instret),
            MISA => {} // WARL: writes ignored, the ISA is fixed
            _ => return false,
        }
        true
    }

    /// Convenience for tests and reset code (counters at zero).
    pub fn write(&mut self, addr: u16, value: u32) -> bool { self.write_with_counters(addr, value, Cycles::ZERO, 0) }
}

fn replace_low(v: u64, low32: u32) -> u64 { (v & !0xffff_ffff) | u64::from(low32) }
fn replace_high(v: u64, high32: u32) -> u64 { (v & 0xffff_ffff) | (u64::from(high32) << 32) }
```
Ajouter à l'interface `MSTATUS_MIE`, `MSTATUS_MPIE`, `write_with_counters` (utilisé par `exec.rs`).

- [ ] **Step 6: Implémenter `hart.rs`**

```rust
//! A single machine-mode RV32IMC hart.
use crate::csr::{CsrFile, MSTATUS_MIE, MSTATUS_MPIE};
use crate::decode::decode;
use crate::trap::{Exception, Trap};
use emu_core::{AccessKind, AccessSize, Bus, Cycles, TimingModel};

const MCAUSE_INTERRUPT: u32 = 1 << 31;
const MTVEC_MODE_MASK: u32 = 0b11;
const MTVEC_VECTORED: u32 = 1;
const COMPRESSED_MASK: u32 = 0b11;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MisalignedPolicy { #[default] Trap, Allow }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct HartConfig { pub misaligned: MisalignedPolicy }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepOutcome { Retired { cycles: u32 }, Trapped { trap: Trap, cycles: u32 }, Interrupted { cause: u32 }, Waiting }

#[derive(Clone, Debug)]
pub struct Hart { pub x: [u32; 32], pub pc: u32, pub csr: CsrFile, pub cycles: Cycles, pub instret: u64, pub config: HartConfig, pub(crate) waiting: bool }

impl Hart {
    #[must_use]
    pub fn new(reset_pc: u32, config: HartConfig) -> Hart {
        Hart { x: [0; 32], pc: reset_pc, csr: CsrFile::default(), cycles: Cycles::ZERO, instret: 0, config, waiting: false }
    }

    pub fn set_interrupt_pending(&mut self, line: u32, pending: bool) {
        let bit = 1u32 << (line & 31);
        if pending { self.csr.mip |= bit } else { self.csr.mip &= !bit }
    }

    /// Highest-numbered line that is pending and enabled (ESP32-C3 priorities are applied by the SoC in SP1b).
    fn pending_line(&self) -> Option<u32> {
        let ready = self.csr.mip & self.csr.mie;
        (ready != 0).then(|| 31 - ready.leading_zeros())
    }

    pub fn step(&mut self, bus: &mut dyn Bus, timing: &dyn TimingModel) -> StepOutcome {
        if self.waiting {
            if self.csr.mip & self.csr.mie == 0 { return StepOutcome::Waiting; }
            self.waiting = false;
        }
        if self.csr.mstatus & MSTATUS_MIE != 0 {
            if let Some(line) = self.pending_line() { self.enter_trap(MCAUSE_INTERRUPT | line, 0); return StepOutcome::Interrupted { cause: line }; }
        }
        let (raw, fetch_stall) = match self.fetch(bus) {
            Ok(v) => v,
            Err(trap) => return self.trap(trap, 1),
        };
        let decoded = decode(raw);
        match self.execute(decoded, bus) {
            Ok(done) => {
                let cycles = timing.instr_cycles(done.class) + fetch_stall + done.stall;
                self.cycles = self.cycles + u64::from(cycles);
                self.instret += 1;
                self.pc = done.next_pc;
                self.waiting = done.wait;
                StepOutcome::Retired { cycles }
            }
            Err(trap) => self.trap(trap, timing.instr_cycles(emu_core::InstrClass::System) + fetch_stall),
        }
    }

    /// Fetches one parcel or two, so a 32-bit instruction may straddle a page or region boundary.
    fn fetch(&self, bus: &mut dyn Bus) -> Result<(u32, u32), Trap> {
        let fault = |_| Trap { cause: Exception::InstrAccessFault, tval: self.pc };
        let lo = bus.read(self.pc, AccessSize::Half, AccessKind::Fetch).map_err(fault)?;
        if lo.value & COMPRESSED_MASK != COMPRESSED_MASK { return Ok((lo.value, lo.stall)); }
        let hi = bus.read(self.pc.wrapping_add(2), AccessSize::Half, AccessKind::Fetch).map_err(fault)?;
        Ok((lo.value | (hi.value << 16), lo.stall + hi.stall))
    }

    fn trap(&mut self, trap: Trap, cycles: u32) -> StepOutcome {
        self.enter_trap(trap.cause.code(), trap.tval);
        self.cycles = self.cycles + u64::from(cycles);
        StepOutcome::Trapped { trap, cycles }
    }

    fn enter_trap(&mut self, mcause: u32, tval: u32) {
        let csr = &mut self.csr;
        csr.mepc = self.pc;
        csr.mcause = mcause;
        csr.mtval = tval;
        let mie_was_set = csr.mstatus & MSTATUS_MIE != 0;
        csr.mstatus &= !(MSTATUS_MIE | MSTATUS_MPIE);
        if mie_was_set { csr.mstatus |= MSTATUS_MPIE; }
        let base = csr.mtvec & !MTVEC_MODE_MASK;
        let vectored = csr.mtvec & MTVEC_MODE_MASK == MTVEC_VECTORED && mcause & MCAUSE_INTERRUPT != 0;
        self.pc = if vectored { base.wrapping_add(4 * (mcause & !MCAUSE_INTERRUPT)) } else { base };
    }

    pub(crate) fn return_from_trap(&mut self) -> u32 {
        let csr = &mut self.csr;
        let mpie = csr.mstatus & MSTATUS_MPIE != 0;
        csr.mstatus = (csr.mstatus & !MSTATUS_MIE) | MSTATUS_MPIE | if mpie { MSTATUS_MIE } else { 0 };
        csr.mepc
    }
}
```

- [ ] **Step 7: Implémenter `exec.rs`**

```rust
//! Semantics of each decoded instruction.
use crate::csr::CsrFile;
use crate::hart::{Hart, MisalignedPolicy};
use crate::instr::{AluOp, BranchOp, CsrOp, Decoded, Instr, LoadOp, MulOp, Reg, StoreOp};
use crate::trap::{Exception, Trap};
use emu_core::{AccessKind, AccessSize, Bus, InstrClass};

pub(crate) struct Done { pub class: InstrClass, pub stall: u32, pub next_pc: u32, pub wait: bool }

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
impl Hart {
    fn r(&self, reg: Reg) -> u32 { self.x[usize::from(reg)] }
    fn w(&mut self, reg: Reg, value: u32) { if reg != 0 { self.x[usize::from(reg)] = value; } }

    pub(crate) fn execute(&mut self, d: Decoded, bus: &mut dyn Bus) -> Result<Done, Trap> {
        let next = self.pc.wrapping_add(u32::from(d.len));
        let done = |class, next_pc| Done { class, stall: 0, next_pc, wait: false };
        Ok(match d.instr {
            Instr::Lui { rd, imm } => { self.w(rd, imm); done(InstrClass::Alu, next) }
            Instr::Auipc { rd, imm } => { self.w(rd, self.pc.wrapping_add(imm)); done(InstrClass::Alu, next) }
            Instr::Jal { rd, offset } => { self.w(rd, next); done(InstrClass::Jump, self.pc.wrapping_add(offset as u32)) }
            Instr::Jalr { rd, rs1, offset } => {
                let target = self.r(rs1).wrapping_add(offset as u32) & !1;
                self.w(rd, next);
                done(InstrClass::Jump, target)
            }
            Instr::Branch { op, rs1, rs2, offset } => {
                let (a, b) = (self.r(rs1), self.r(rs2));
                let taken = match op {
                    BranchOp::Eq => a == b, BranchOp::Ne => a != b,
                    BranchOp::Lt => (a as i32) < (b as i32), BranchOp::Ge => (a as i32) >= (b as i32),
                    BranchOp::Ltu => a < b, BranchOp::Geu => a >= b,
                };
                if taken { done(InstrClass::BranchTaken, self.pc.wrapping_add(offset as u32)) } else { done(InstrClass::BranchNotTaken, next) }
            }
            Instr::Load { op, rd, rs1, offset } => {
                let addr = self.r(rs1).wrapping_add(offset as u32);
                let size = match op { LoadOp::B | LoadOp::Bu => AccessSize::Byte, LoadOp::H | LoadOp::Hu => AccessSize::Half, LoadOp::W => AccessSize::Word };
                self.check_alignment(addr, size, Exception::LoadMisaligned)?;
                let out = bus.read(addr, size, AccessKind::Load).map_err(|_| Trap { cause: Exception::LoadAccessFault, tval: addr })?;
                let value = match op {
                    LoadOp::B => out.value as u8 as i8 as i32 as u32,
                    LoadOp::H => out.value as u16 as i16 as i32 as u32,
                    LoadOp::W | LoadOp::Bu | LoadOp::Hu => out.value,
                };
                self.w(rd, value);
                Done { class: InstrClass::Load, stall: out.stall, next_pc: next, wait: false }
            }
            Instr::Store { op, rs1, rs2, offset } => {
                let addr = self.r(rs1).wrapping_add(offset as u32);
                let size = match op { StoreOp::B => AccessSize::Byte, StoreOp::H => AccessSize::Half, StoreOp::W => AccessSize::Word };
                self.check_alignment(addr, size, Exception::StoreMisaligned)?;
                let stall = bus.write(addr, size, self.r(rs2)).map_err(|_| Trap { cause: Exception::StoreAccessFault, tval: addr })?;
                Done { class: InstrClass::Store, stall, next_pc: next, wait: false }
            }
            Instr::OpImm { op, rd, rs1, imm } => { let v = alu(op, self.r(rs1), imm as u32); self.w(rd, v); done(InstrClass::Alu, next) }
            Instr::Op { op, rd, rs1, rs2 } => { let v = alu(op, self.r(rs1), self.r(rs2)); self.w(rd, v); done(InstrClass::Alu, next) }
            Instr::MulDiv { op, rd, rs1, rs2 } => {
                let v = muldiv(op, self.r(rs1), self.r(rs2));
                self.w(rd, v);
                let class = if matches!(op, MulOp::Mul | MulOp::Mulh | MulOp::Mulhsu | MulOp::Mulhu) { InstrClass::Mul } else { InstrClass::Div };
                done(class, next)
            }
            Instr::Csr { op, rd, src, uimm, csr } => { self.exec_csr(d, op, rd, src, uimm, csr)?; done(InstrClass::Csr, next) }
            Instr::Fence | Instr::FenceI => done(InstrClass::System, next),
            Instr::Ecall => return Err(Trap { cause: Exception::EcallM, tval: 0 }),
            Instr::Ebreak => return Err(Trap { cause: Exception::Breakpoint, tval: self.pc }),
            Instr::Mret => { let target = self.return_from_trap(); done(InstrClass::System, target) }
            Instr::Wfi => Done { class: InstrClass::System, stall: 0, next_pc: next, wait: true },
            Instr::Illegal(raw) => return Err(Trap { cause: Exception::IllegalInstr, tval: raw }),
        })
    }

    fn check_alignment(&self, addr: u32, size: AccessSize, cause: Exception) -> Result<(), Trap> {
        let misaligned = addr % size.bytes() != 0;
        if misaligned && self.config.misaligned == MisalignedPolicy::Trap { return Err(Trap { cause, tval: addr }); }
        Ok(())
    }

    fn exec_csr(&mut self, d: Decoded, op: CsrOp, rd: Reg, src: u8, uimm: bool, csr: u16) -> Result<(), Trap> {
        let illegal = Trap { cause: Exception::IllegalInstr, tval: d.raw };
        let operand = if uimm { u32::from(src) } else { self.r(src) };
        let old = self.csr.read(csr, self.cycles, self.instret).ok_or(illegal)?;
        // csrrs/csrrc with a zero source never write (and may target read-only CSRs).
        let writes = op == CsrOp::Rw || src != 0;
        if writes {
            let new = match op { CsrOp::Rw => operand, CsrOp::Rs => old | operand, CsrOp::Rc => old & !operand };
            if !self.csr.write_with_counters(csr, new, self.cycles, self.instret) { return Err(illegal); }
        }
        self.w(rd, old);
        Ok(())
    }
}

fn alu(op: AluOp, a: u32, b: u32) -> u32 {
    let shamt = b & 0x1f;
    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    match op {
        AluOp::Add => a.wrapping_add(b), AluOp::Sub => a.wrapping_sub(b),
        AluOp::Sll => a << shamt, AluOp::Srl => a >> shamt, AluOp::Sra => ((a as i32) >> shamt) as u32,
        AluOp::Slt => u32::from((a as i32) < (b as i32)), AluOp::Sltu => u32::from(a < b),
        AluOp::Xor => a ^ b, AluOp::Or => a | b, AluOp::And => a & b,
    }
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn muldiv(op: MulOp, a: u32, b: u32) -> u32 {
    let (sa, sb) = (a as i32, b as i32);
    match op {
        MulOp::Mul => a.wrapping_mul(b),
        MulOp::Mulh => ((i64::from(sa) * i64::from(sb)) >> 32) as u32,
        MulOp::Mulhsu => ((i64::from(sa) * i64::from(b)) >> 32) as u32,
        MulOp::Mulhu => ((u64::from(a) * u64::from(b)) >> 32) as u32,
        MulOp::Div => if b == 0 { u32::MAX } else { sa.wrapping_div(sb) as u32 },
        MulOp::Divu => a.checked_div(b).unwrap_or(u32::MAX),
        MulOp::Rem => if b == 0 { a } else { sa.wrapping_rem(sb) as u32 },
        MulOp::Remu => if b == 0 { a } else { a % b },
    }
}
```
Ajouter aussi ce test dans le module de tests de `hart.rs` : `mtval` d'un accès CSR illégal doit valoir le mot de l'instruction (porté par `Decoded::raw`).
```rust
    #[test]
    fn illegal_csr_access_reports_instruction_word() {
        let word = csrrw(1, MHARTID, 2);
        let (mut h, mut m) = machine(&[word], MisalignedPolicy::Trap);
        let out = run(&mut h, &mut m, 1);
        assert_eq!(out[0], StepOutcome::Trapped { trap: Trap { cause: Exception::IllegalInstr, tval: word }, cycles: 1 });
    }
```

`lib.rs` : ajouter `pub mod csr; pub mod hart; pub mod trap; mod exec; #[cfg(test)] pub(crate) mod asm;` et `pub use hart::{Hart, HartConfig, MisalignedPolicy, StepOutcome}; pub use trap::{Exception, Trap};`.

- [ ] **Step 8: Lancer les tests**

Run: `cargo test -p cpu-rv32 && cargo clippy -p cpu-rv32 --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git commit -m "feat(rv32): machine-mode hart with CSRs, traps, vectored interrupts and WFI" -- crates/cpu-rv32
```

---

### Task 6: Cache de décodage par page, sans perte du coût de fetch

Un succès de cache évite de relire et redécoder l'instruction, mais **pas** son coût temporel : sur l'ESP32-C3, le coût d'un fetch dépend du cache flash, que le SoC modélisera en SP1b. On ajoute donc au `Bus` un crochet `fetch_stall`, qui ne transfère aucune donnée et ne fait que compter le temps. L'implémentation par défaut renvoie 0.

Une `HashMap` est utilisée pour les recherches de page ; elle n'est jamais itérée, ce qui respecte la contrainte de déterminisme.

**Files:**
- Modify: `crates/emu-core/src/bus.rs` (méthode `fetch_stall`)
- Create: `crates/cpu-rv32/src/dcache.rs`
- Modify: `crates/cpu-rv32/src/hart.rs` (champ `dcache`, `fetch` passe par le cache)

**Interfaces:**
- Produces :
  - `Bus::fetch_stall(&mut self, addr: u32, len: u32) -> u32` (défaut : `0`).
  - `DecodeCache::new()`, `fn lookup(&mut self, pc: u32, bus: &dyn Bus) -> Option<Decoded>`, `fn insert(&mut self, pc: u32, decoded: Decoded, bus: &dyn Bus)`, `fn stats(&self) -> (u64 /*hits*/, u64 /*misses*/)`.

- [ ] **Step 1: Tests (échouent)**

En bas de `dcache.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode;
    use emu_core::{AccessSize, Bus, Ram};

    #[test]
    fn hit_after_insert_then_miss_after_write_to_same_page() {
        let mut ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        assert!(cache.lookup(0x10, &ram).is_none());
        cache.insert(0x10, decode(0x0050_0093), &ram);
        assert_eq!(cache.lookup(0x10, &ram).map(|d| d.raw), Some(0x0050_0093));
        ram.write(0x800, AccessSize::Word, 0).unwrap();                // same 4 KiB page
        assert!(cache.lookup(0x10, &ram).is_none());
    }

    #[test]
    fn write_to_other_page_keeps_entry() {
        let mut ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        cache.insert(0x10, decode(0x0050_0093), &ram);
        ram.write(0x1000, AccessSize::Word, 0).unwrap();
        assert!(cache.lookup(0x10, &ram).is_some());
    }

    #[test]
    fn instruction_straddling_a_page_is_never_cached() {
        let ram = Ram::new(0, 8192);
        let mut cache = DecodeCache::new();
        cache.insert(0xffe, decode(0x0050_0093), &ram);
        assert!(cache.lookup(0xffe, &ram).is_none());
    }
}
```
Dans le module de tests de `hart.rs`, ajouter (le mini-assembleur et `machine` existent déjà) :
```rust
    #[test]
    fn store_into_decoded_page_invalidates() {
        // The program runs the instruction at 16 once (caching it), overwrites it with
        // `addi x5,x0,7`, then jumps back to it. Execution order: 0,4,8,12,16,20,24,16 = 8 steps.
        let patched = addi(5, 0, 7);
        let (mut h, mut m) = machine(&[
            addi(5, 0, 1),                    // 0
            lui(2, patched.wrapping_add(0x800) & 0xffff_f000),   // 4
            addi(2, 2, ((patched & 0xfff) as i32) << 20 >> 20),  // 8: x2 = patched word
            jal(0, 4),                        // 12: skip to 16
            addi(5, 0, 1),                    // 16: original instruction (executed first)
            sw(2, 0, 16),                     // 20: overwrite instruction at 16
            jal(0, -8),                       // 24: back to 16
        ], MisalignedPolicy::Trap);
        run(&mut h, &mut m, 8);
        assert_eq!(h.x[5], 7, "the rewritten instruction must run, not the cached one");
    }

    #[test]
    fn cache_hit_still_pays_fetch_stall() {
        #[derive(Debug)]
        struct SlowFetch(Ram);
        impl Bus for SlowFetch {
            fn read(&mut self, a: u32, s: AccessSize, k: AccessKind) -> Result<emu_core::ReadOutcome, emu_core::BusError> {
                let mut o = self.0.read(a, s, k)?; if k == AccessKind::Fetch { o.stall = 3; } Ok(o)
            }
            fn write(&mut self, a: u32, s: AccessSize, v: u32) -> Result<u32, emu_core::BusError> { self.0.write(a, s, v) }
            fn page_version(&self, a: u32) -> u64 { self.0.page_version(a) }
            fn fetch_stall(&mut self, _a: u32, _len: u32) -> u32 { 3 }
        }
        let (mut h, m) = machine(&[jal(0, 0)], MisalignedPolicy::Trap);   // infinite self-loop
        let mut bus = SlowFetch(m);
        let first = h.step(&mut bus, &OneCycle);
        let second = h.step(&mut bus, &OneCycle);
        assert_eq!(first, StepOutcome::Retired { cycles: 1 + 6 });      // two 16-bit fetch reads at 3 each
        assert_eq!(second, StepOutcome::Retired { cycles: 1 + 3 });     // cached: one fetch_stall call
        assert_eq!(h.dcache.stats(), (1, 1));
    }
```
L'immédiat de `addi(2, 2, …)` est l'extension de signe des 12 bits bas du mot ; `lui` reçoit `patched + 0x800` arrondi, pour compenser cette extension (technique standard `%hi/%lo`).

- [ ] **Step 2: Constater l'échec**

Run: `cargo test -p cpu-rv32 -p emu-core`
Expected: erreurs de compilation (`DecodeCache`, `fetch_stall`, `dcache` inexistants).

- [ ] **Step 3: Ajouter `fetch_stall` au trait `Bus`**

Dans `bus.rs`, à la fin du trait :
```rust
    /// Timing-only hook called when an instruction is served from a decode cache:
    /// the SoC charges what the fetch would have cost (e.g. flash-cache hit or miss) without moving data.
    fn fetch_stall(&mut self, addr: u32, len: u32) -> u32 { let _ = (addr, len); 0 }
```

- [ ] **Step 4: Implémenter `dcache.rs`**

```rust
//! Per-page decoded-instruction cache, invalidated through `Bus::page_version`.
use crate::instr::Decoded;
use emu_core::{Bus, PAGE_SHIFT};
use std::collections::HashMap;

const PAGE_SIZE: u32 = 1 << PAGE_SHIFT;
/// Instructions are 2-byte aligned with the C extension.
const SLOTS_PER_PAGE: usize = (PAGE_SIZE / 2) as usize;
const MAX_INSTR_BYTES: u32 = 4;

#[derive(Debug)]
struct Page { version: u64, slots: Box<[Option<Decoded>]> }

#[derive(Debug, Default)]
pub struct DecodeCache { pages: HashMap<u32, Page>, hits: u64, misses: u64 }

impl DecodeCache {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    fn slot(pc: u32) -> usize { ((pc & (PAGE_SIZE - 1)) >> 1) as usize }

    pub fn lookup(&mut self, pc: u32, bus: &dyn Bus) -> Option<Decoded> {
        let found = self.pages.get(&(pc >> PAGE_SHIFT))
            .filter(|p| p.version == bus.page_version(pc))
            .and_then(|p| p.slots[Self::slot(pc)]);
        if found.is_some() { self.hits += 1 } else { self.misses += 1 }
        found
    }

    pub fn insert(&mut self, pc: u32, decoded: Decoded, bus: &dyn Bus) {
        // An instruction crossing into the next page would need two versions; it is rare, so it is not cached.
        if (pc & (PAGE_SIZE - 1)) + u32::from(decoded.len) > PAGE_SIZE { return; }
        let version = bus.page_version(pc);
        let page = self.pages.entry(pc >> PAGE_SHIFT)
            .or_insert_with(|| Page { version, slots: vec![None; SLOTS_PER_PAGE].into_boxed_slice() });
        if page.version != version { page.slots.fill(None); page.version = version; }
        page.slots[Self::slot(pc)] = Some(decoded);
    }

    #[must_use]
    pub fn stats(&self) -> (u64, u64) { (self.hits, self.misses) }
}

const _: () = assert!(MAX_INSTR_BYTES <= PAGE_SIZE);
```
`MAX_INSTR_BYTES` documente l'hypothèse ; si clippy le signale inutilisé hors de l'assertion, le garder : l'assertion de compilation l'utilise.

- [ ] **Step 5: Brancher le cache dans le hart**

Dans `hart.rs` : ajouter le champ `pub(crate) dcache: DecodeCache` (initialisé par `DecodeCache::new()` dans `Hart::new`) et rendre `dcache` lisible pour les tests (`pub(crate)` suffit, les tests sont dans le module). Remplacer, dans `step`, la séquence `fetch` + `decode` par :
```rust
        let (decoded, fetch_stall) = match self.dcache.lookup(self.pc, bus) {
            Some(d) => (d, bus.fetch_stall(self.pc, u32::from(d.len))),
            None => match self.fetch(bus) {
                Ok((raw, stall)) => { let d = decode(raw); self.dcache.insert(self.pc, d, bus); (d, stall) }
                Err(trap) => return self.trap(trap, 1),
            },
        };
        match self.execute(decoded, bus) {
```
Comme `lookup` prend `&dyn Bus` et `bus` est `&mut dyn Bus`, passer `&*bus`.

- [ ] **Step 6: Lancer les tests**

Run: `cargo test -p cpu-rv32 -p emu-core && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git commit -m "feat(rv32): page-versioned decode cache that keeps fetch timing" -- crates/emu-core crates/cpu-rv32
```

---

### Task 7: Modèle de timing initial `DatasheetTiming`

Espressif ne publie pas les latences par instruction du cœur de l'ESP32-C3 (voir spec §1.3). Les valeurs ci-dessous sont des **estimations de départ** pour un cœur RISC-V à pipeline court mono-émission. Elles seront ajustées en SP1b par le test de tolérance CoreMark. Le modèle se déclare donc non calibré.

**Files:**
- Create: `crates/cpu-rv32/src/timing.rs`
- Modify: `crates/cpu-rv32/src/lib.rs`

**Interfaces:**
- Consumes: `emu_core::{InstrClass, TimingModel}`.
- Produces: `DatasheetTiming` (`Default`), `name()` = `"datasheet-uncalibrated"`, `is_calibrated()` = `false`.

- [ ] **Step 1: Test (échoue)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::{InstrClass, TimingModel};
    #[test]
    fn model_is_explicitly_uncalibrated_and_plausible() {
        let t = DatasheetTiming::default();
        assert_eq!(t.name(), "datasheet-uncalibrated");
        assert!(!t.is_calibrated());
        assert!(t.instr_cycles(InstrClass::BranchTaken) > t.instr_cycles(InstrClass::BranchNotTaken));
        assert!(t.instr_cycles(InstrClass::Div) > t.instr_cycles(InstrClass::Mul));
        for c in [InstrClass::Alu, InstrClass::Mul, InstrClass::Div, InstrClass::Load, InstrClass::Store,
                  InstrClass::BranchTaken, InstrClass::BranchNotTaken, InstrClass::Jump, InstrClass::Csr, InstrClass::System] {
            assert!(t.instr_cycles(c) >= 1);
        }
    }
}
```

- [ ] **Step 2: Constater l'échec** — Run: `cargo test -p cpu-rv32 timing` → erreur de compilation.

- [ ] **Step 3: Implémenter**

```rust
//! Initial, uncalibrated instruction costs (spec §4.2). Memory wait states are added by the bus.
use emu_core::{InstrClass, TimingModel};

/// Single-issue short pipeline: one cycle per simple instruction.
const ALU: u32 = 1;
/// Loads/stores: one cycle in the pipeline; SRAM/flash wait states come from the bus.
const LOAD: u32 = 1;
const STORE: u32 = 1;
/// Taken branches and jumps refill the front of the pipeline (estimate: 2-cycle penalty).
const BRANCH_TAKEN: u32 = 3;
const BRANCH_NOT_TAKEN: u32 = 1;
const JUMP: u32 = 3;
/// Estimates pending CoreMark fitting in SP1b.
const MUL: u32 = 2;
const DIV: u32 = 33;
const CSR: u32 = 1;
const SYSTEM: u32 = 3;

#[derive(Debug, Default, Clone, Copy)]
pub struct DatasheetTiming;

impl TimingModel for DatasheetTiming {
    fn name(&self) -> &'static str { "datasheet-uncalibrated" }
    fn is_calibrated(&self) -> bool { false }
    fn instr_cycles(&self, class: InstrClass) -> u32 {
        match class {
            InstrClass::Alu => ALU, InstrClass::Load => LOAD, InstrClass::Store => STORE,
            InstrClass::BranchTaken => BRANCH_TAKEN, InstrClass::BranchNotTaken => BRANCH_NOT_TAKEN,
            InstrClass::Jump => JUMP, InstrClass::Mul => MUL, InstrClass::Div => DIV,
            InstrClass::Csr => CSR, InstrClass::System => SYSTEM,
        }
    }
}
```
`lib.rs` : `pub mod timing; pub use timing::DatasheetTiming;`.

- [ ] **Step 4: Lancer les tests** — Run: `cargo test -p cpu-rv32` → PASS.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(rv32): initial uncalibrated datasheet timing model" -- crates/cpu-rv32
```

---

### Task 8: Machine de test `rvtest` et suite `riscv-tests` (rv32ui/um/uc/mi)

`riscv-tests` (dépôt `riscv-software-src/riscv-tests`, licence BSD-3) contient des tests **auto-vérifiants** : chacun écrit son verdict dans le symbole `tohost` (`1` = succès, sinon `(numéro du test en échec << 1) | 1`). Ils couvrent aussi le mode machine (`rv32mi`) et `fence.i`, qui valide le cache de décodage de la Task 6.

Les ELF compilés sont **committés** sous `tests/fixtures/riscv-tests/` avec un manifeste SHA-256. Ainsi la CI n'a pas besoin de Docker, et un script les régénère de façon reproductible. Le compilateur GPL ne sert qu'à produire ces fixtures ; il n'est ni lié ni distribué.

**Files:**
- Create: `crates/emu-cli/src/lib.rs`, `crates/emu-cli/src/rvtest.rs`
- Modify: `crates/emu-cli/src/main.rs`, `crates/emu-cli/Cargo.toml`
- Create: `crates/emu-cli/tests/riscv_tests.rs`
- Create: `tools/docker/riscv-gcc/Dockerfile`, `tools/conformance/versions.env`, `tools/conformance/build-riscv-tests.sh`, `tools/conformance/riscv-tests-skip.txt`
- Create: `tests/fixtures/riscv-tests/*` (générés) + `tests/fixtures/riscv-tests/SHA256SUMS`

**Interfaces:**
- Consumes: `cpu_rv32::{Hart, HartConfig, MisalignedPolicy, StepOutcome, DatasheetTiming}`, `emu_core::{Bus, Ram, TraceHash, ...}`, `goblin`.
- Produces (bibliothèque `emu_cli::rvtest`) :
  - `TestMachine::from_elf(bytes: &[u8], config: HartConfig) -> Result<TestMachine, RvTestError>`
  - `fn run(&mut self, max_steps: u64) -> RunResult` où `enum RunResult { Passed, Failed { test_number: u32 }, Timeout }` (`Passed` = `tohost` vaut 1, ce qui est la fin normale dans les deux protocoles).
  - `fn signature(&self) -> Result<Vec<u32>, RvTestError>` (mots entre `begin_signature` et `end_signature`).
  - `fn trace_hash(&self) -> u64` (empreinte (pc, mot d'instruction) de chaque instruction retirée).
- Produces (CLI) : `inkemu rvtest <ELF> [--max-steps N] [--signature FILE] [--trace-hash] [--allow-misaligned]` ; codes de sortie `0` succès, `1` échec, `2` timeout ou erreur.

- [ ] **Step 1: Toolchain Docker et script de build**

`tools/docker/riscv-gcc/Dockerfile` :
```dockerfile
# Test-fixture toolchain only (GPL): never linked into or shipped with inkemu.
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-riscv64-unknown-elf binutils-riscv64-unknown-elf \
      make autoconf git ca-certificates python3 python3-venv \
 && rm -rf /var/lib/apt/lists/*
```
Épingler le commit : `git ls-remote https://github.com/riscv-software-src/riscv-tests HEAD`, puis écrire dans `tools/conformance/versions.env` :
```bash
# Pinned sources for conformance fixtures. Bump in a dedicated PR.
RISCV_TESTS_COMMIT=<sha40 affiché par la commande ci-dessus>
```
`tools/conformance/build-riscv-tests.sh` :
```bash
#!/usr/bin/env bash
# Rebuilds tests/fixtures/riscv-tests from the pinned riscv-tests commit.
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
out=tests/fixtures/riscv-tests
docker build -t inkemu/riscv-gcc tools/docker/riscv-gcc
rm -rf "$out" && mkdir -p "$out"
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD/$out:/out" -w /tmp inkemu/riscv-gcc bash -euo pipefail -c "
  git clone -q https://github.com/riscv-software-src/riscv-tests src
  cd src && git checkout -q $RISCV_TESTS_COMMIT && git submodule update -q --init env
  cd isa && make -s XLEN=32 RISCV_PREFIX=riscv64-unknown-elf- rv32ui rv32um rv32uc rv32mi
  for f in rv32ui-p-* rv32um-p-* rv32uc-p-* rv32mi-p-*; do
    case \$f in *.dump) ;; *) cp \"\$f\" /out/ ;; esac
  done"
(cd "$out" && sha256sum -- * > SHA256SUMS)
echo "built $(ls "$out" | wc -l) files into $out"
```
`chmod +x tools/conformance/build-riscv-tests.sh`.

- [ ] **Step 2: Générer les fixtures**

Run: `tools/conformance/build-riscv-tests.sh`
Expected: `built N files` avec N > 100.
Si `make` refuse une cible (`No rule to make target 'rv32ui'`), remplacer la liste de cibles par `make -s XLEN=32 RISCV_PREFIX=riscv64-unknown-elf-` (cible par défaut, qui construit tous les tests `rv32*`), la boucle de copie filtrant déjà les suites voulues. Si GCC rejette `csrr`/`fence.i` (« extension `zicsr' required »), ajouter `RISCV_GCC_OPTS='-static -mcmodel=medany -fvisibility=hidden -nostdlib -nostartfiles -march=rv32g_zicsr_zifencei'` à la ligne `make`, puis relancer.

- [ ] **Step 3: Tests d'intégration (échouent)**

`crates/emu-cli/tests/riscv_tests.rs` :
```rust
//! Runs every committed riscv-tests ELF; any failure outside the documented skip list fails CI.
use cpu_rv32::HartConfig;
use emu_cli::rvtest::{RunResult, TestMachine};
use std::path::PathBuf;

const MAX_STEPS: u64 = 5_000_000;

fn repo_root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..") }

fn skip_list() -> Vec<String> {
    let text = std::fs::read_to_string(repo_root().join("tools/conformance/riscv-tests-skip.txt")).unwrap();
    text.lines().filter_map(|l| l.split('#').next()).map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
}

fn fixtures() -> Vec<PathBuf> {
    let dir = repo_root().join("tests/fixtures/riscv-tests");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rv32"))).collect();
    v.sort();
    v
}

#[test]
fn all_riscv_tests_pass_except_documented_skips() {
    let skips = skip_list();
    let files = fixtures();
    assert!(files.len() > 100, "fixtures missing: run tools/conformance/build-riscv-tests.sh");
    let mut failures = Vec::new();
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if skips.contains(&name) { continue; }
        let mut m = TestMachine::from_elf(&std::fs::read(path).unwrap(), HartConfig::default()).unwrap();
        match m.run(MAX_STEPS) {
            RunResult::Passed => {}
            other => failures.push(format!("{name}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{} failing:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn skip_list_entries_exist_and_have_reasons() {
    let text = std::fs::read_to_string(repo_root().join("tools/conformance/riscv-tests-skip.txt")).unwrap();
    let names: Vec<String> = fixtures().iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (name, reason) = line.split_once('#').unwrap_or((line, ""));
        assert!(names.contains(&name.trim().to_string()), "unknown test in skip list: {name}");
        assert!(reason.trim().len() > 10, "skip entry needs a reason citing the spec: {line}");
    }
}

#[test]
fn same_test_twice_gives_same_trace_hash() {
    let path = repo_root().join("tests/fixtures/riscv-tests/rv32ui-p-add");
    let bytes = std::fs::read(path).unwrap();
    let run = || { let mut m = TestMachine::from_elf(&bytes, HartConfig::default()).unwrap(); m.run(MAX_STEPS); m.trace_hash() };
    assert_eq!(run(), run());
}
```
`tools/conformance/riscv-tests-skip.txt` (contenu initial) :
```
# One test per line: <file name> # <reason, citing the spec section that puts the feature out of scope>
rv32ui-p-ma_data # misaligned data accesses trap by default (MisalignedPolicy::Trap); the C3 behaviour is arbitrated by the QEMU lockstep in SP1b (spec §4.4)
```

- [ ] **Step 4: Constater l'échec**

Run: `cargo test -p emu-cli --test riscv_tests`
Expected: erreur de compilation (`emu_cli::rvtest` inexistant).

- [ ] **Step 5: Implémenter `rvtest.rs`**

```rust
//! Minimal test machine for riscv-tests and riscv-arch-test: RAM at 0x8000_0000 plus a `tohost` mailbox.
use cpu_rv32::{DatasheetTiming, Hart, HartConfig, StepOutcome};
use emu_core::{AccessKind, AccessSize, Bus, BusError, Ram, ReadOutcome, TraceHash};
use goblin::elf::{program_header::PT_LOAD, Elf};
use thiserror::Error;

/// Link address used by riscv-tests and riscv-arch-test (`link.ld`).
pub const RAM_BASE: u32 = 0x8000_0000;
pub const RAM_SIZE: usize = 4 * 1024 * 1024;
const TOHOST_PASS: u32 = 1;

#[derive(Debug, Error)]
pub enum RvTestError {
    #[error("not a valid ELF file: {0}")]
    Elf(#[from] goblin::error::Error),
    #[error("ELF is not 32-bit RISC-V; build the test with -march=rv32... -mabi=ilp32")]
    WrongArch,
    #[error("ELF has no `{0}` symbol; the test must be linked with the riscv-tests/arch-test environment")]
    MissingSymbol(&'static str),
    #[error("ELF segment at {addr:#010x} does not fit in the {RAM_SIZE}-byte test RAM at {RAM_BASE:#010x}")]
    SegmentOutOfRange { addr: u32 },
    #[error("signature region is outside test RAM")]
    SignatureOutOfRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunResult { Passed, Failed { test_number: u32 }, Timeout }

#[derive(Debug)]
struct HostBus { ram: Ram, tohost: u32, written: Option<u32> }

impl Bus for HostBus {
    fn read(&mut self, a: u32, s: AccessSize, k: AccessKind) -> Result<ReadOutcome, BusError> { self.ram.read(a, s, k) }
    fn write(&mut self, a: u32, s: AccessSize, v: u32) -> Result<u32, BusError> {
        if a == self.tohost && v != 0 { self.written = Some(v); }
        self.ram.write(a, s, v)
    }
    fn page_version(&self, a: u32) -> u64 { self.ram.page_version(a) }
}

#[derive(Debug)]
pub struct TestMachine { hart: Hart, bus: HostBus, sig: Option<(u32, u32)>, trace: TraceHash }

impl TestMachine {
    /// # Errors
    /// See [`RvTestError`].
    pub fn from_elf(bytes: &[u8], config: HartConfig) -> Result<TestMachine, RvTestError> {
        let elf = Elf::parse(bytes)?;
        if elf.is_64 || elf.header.e_machine != goblin::elf::header::EM_RISCV { return Err(RvTestError::WrongArch); }
        let mut ram = Ram::new(RAM_BASE, RAM_SIZE);
        for ph in elf.program_headers.iter().filter(|p| p.p_type == PT_LOAD && p.p_filesz > 0) {
            let addr = u32::try_from(ph.p_paddr).map_err(|_| RvTestError::WrongArch)?;
            let start = usize::try_from(ph.p_offset).map_err(|_| RvTestError::SegmentOutOfRange { addr })?;
            let len = usize::try_from(ph.p_filesz).map_err(|_| RvTestError::SegmentOutOfRange { addr })?;
            let data = bytes.get(start..start + len).ok_or(RvTestError::SegmentOutOfRange { addr })?;
            ram.load(addr, data).map_err(|_| RvTestError::SegmentOutOfRange { addr })?;
        }
        let symbol = |name: &str| elf.syms.iter().find(|s| elf.strtab.get_at(s.st_name) == Some(name)).and_then(|s| u32::try_from(s.st_value).ok());
        let tohost = symbol("tohost").ok_or(RvTestError::MissingSymbol("tohost"))?;
        let sig = symbol("begin_signature").zip(symbol("end_signature"));
        let entry = u32::try_from(elf.entry).map_err(|_| RvTestError::WrongArch)?;
        Ok(TestMachine { hart: Hart::new(entry, config), bus: HostBus { ram, tohost, written: None }, sig, trace: TraceHash::new() })
    }

    pub fn run(&mut self, max_steps: u64) -> RunResult {
        let timing = DatasheetTiming;
        for _ in 0..max_steps {
            let pc = self.hart.pc;
            if let StepOutcome::Retired { .. } = self.hart.step(&mut self.bus, &timing) {
                self.trace.mix_u32(pc);
                self.trace.mix_u32(self.bus.ram.bytes(pc, 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            }
            if let Some(v) = self.bus.written {
                // Both riscv-tests (pass) and riscv-arch-test (RVMODEL_HALT) write 1 for a normal end.
                return if v == TOHOST_PASS { RunResult::Passed } else { RunResult::Failed { test_number: v >> 1 } };
            }
        }
        RunResult::Timeout
    }

    /// # Errors
    /// [`RvTestError::MissingSymbol`] if the ELF has no signature region.
    pub fn signature(&self) -> Result<Vec<u32>, RvTestError> {
        let (begin, end) = self.sig.ok_or(RvTestError::MissingSymbol("begin_signature"))?;
        let len = usize::try_from(end.saturating_sub(begin)).map_err(|_| RvTestError::SignatureOutOfRange)?;
        let bytes = self.bus.ram.bytes(begin, len).map_err(|_| RvTestError::SignatureOutOfRange)?;
        Ok(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }

    #[must_use]
    pub fn trace_hash(&self) -> u64 { self.trace.finish() }
}
```
La lecture du mot pour l'empreinte peut déborder de 2 octets en fin de RAM pour une compressée en toute dernière position : `map_or(0, …)` couvre ce cas sans faux échec.

`crates/emu-cli/src/lib.rs` :
```rust
//! Library side of the inkemu CLI, so integration tests can drive the same code as the binary.
pub mod rvtest;
```
Ajouter `thiserror.workspace = true` aux dépendances de `emu-cli`.

- [ ] **Step 6: Sous-commande `rvtest`**

`main.rs` :
```rust
//! inkemu command-line interface.
use clap::{Parser, Subcommand};
use cpu_rv32::{HartConfig, MisalignedPolicy};
use emu_cli::rvtest::{RunResult, TestMachine};
use std::path::PathBuf;
use std::process::ExitCode;

const EXIT_FAIL: u8 = 1;
const EXIT_ERROR: u8 = 2;

#[derive(Parser)]
#[command(name = "inkemu", version, about = "Binary-level emulator for embedded e-ink devices")]
struct Cli { #[command(subcommand)] command: Command }

#[derive(Subcommand)]
enum Command {
    /// Run a bare-metal RISC-V conformance test ELF (riscv-tests or riscv-arch-test).
    Rvtest {
        elf: PathBuf,
        #[arg(long, default_value_t = 50_000_000)]
        max_steps: u64,
        /// Write the arch-test signature (one 8-digit hex word per line) to this file.
        #[arg(long)]
        signature: Option<PathBuf>,
        /// Print the execution trace hash (determinism checks).
        #[arg(long)]
        trace_hash: bool,
        #[arg(long)]
        allow_misaligned: bool,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Rvtest { elf, max_steps, signature, trace_hash, allow_misaligned } =>
            run_rvtest(&elf, max_steps, signature.as_deref(), trace_hash, allow_misaligned),
    }
}

fn run_rvtest(elf: &std::path::Path, max_steps: u64, signature: Option<&std::path::Path>, trace_hash: bool, allow_misaligned: bool) -> ExitCode {
    let config = HartConfig { misaligned: if allow_misaligned { MisalignedPolicy::Allow } else { MisalignedPolicy::Trap } };
    let machine = std::fs::read(elf).map_err(|e| format!("cannot read {}: {e}", elf.display()))
        .and_then(|b| TestMachine::from_elf(&b, config).map_err(|e| e.to_string()));
    let mut machine = match machine { Ok(m) => m, Err(e) => { eprintln!("error: {e}"); return ExitCode::from(EXIT_ERROR); } };
    let result = machine.run(max_steps);
    if trace_hash { println!("trace-hash {:016x}", machine.trace_hash()); }
    if let Some(path) = signature {
        let words = match machine.signature() { Ok(w) => w, Err(e) => { eprintln!("error: {e}"); return ExitCode::from(EXIT_ERROR); } };
        let text: String = words.iter().map(|w| format!("{w:08x}\n")).collect();
        if let Err(e) = std::fs::write(path, text) { eprintln!("error: cannot write {}: {e}", path.display()); return ExitCode::from(EXIT_ERROR); }
    }
    match result {
        RunResult::Passed => { println!("Passed"); ExitCode::SUCCESS }
        RunResult::Failed { test_number } => { println!("FAILED at test #{test_number}"); ExitCode::from(EXIT_FAIL) }
        RunResult::Timeout => { eprintln!("error: no verdict after {max_steps} steps"); ExitCode::from(EXIT_ERROR) }
    }
}
```

- [ ] **Step 7: Faire passer la suite**

Run: `cargo test -p emu-cli --test riscv_tests -- --nocapture`
Expected: PASS. Pour chaque test en échec, la règle est de **corriger le cœur** en s'appuyant sur le numéro de sous-test et le désassemblage (`riscv64-unknown-elf-objdump -d` dans l'image Docker). Un test ne va dans la liste d'exclusion que si la fonctionnalité est hors périmètre selon la spec, avec la section citée. Exemples attendus : U-mode absent (spec : mode machine seul), déclencheurs de débogage (`tselect`) absents.

- [ ] **Step 8: Commit**

```bash
git add tools/docker/riscv-gcc tools/conformance tests/fixtures/riscv-tests
git commit -m "test(rv32): riscv-tests conformance suite with rvtest machine and CLI" -- crates/emu-cli tools/docker/riscv-gcc tools/conformance tests/fixtures/riscv-tests
```

---

### Task 9: Conformité `riscv-arch-test` via RISCOF (référence Sail)

Contrairement à `riscv-tests`, les tests d'architecture ne s'auto-vérifient pas. Chaque test écrit une **signature** (zone mémoire entre `begin_signature` et `end_signature`). RISCOF compile le test, l'exécute sur inkemu et sur le modèle formel Sail, puis compare les signatures. On épingle la **dernière version 3.x** de `riscv-arch-test`, la lignée qui utilise RISCOF.

Tout tourne dans Docker (`tools/docker/riscof`) : Python, RISCOF, Sail et GCC sont des outils de test, jamais distribués.

**Files:**
- Create: `tools/docker/riscof/Dockerfile`
- Create: `tools/conformance/riscof/config.ini`
- Create: `tools/conformance/riscof/inkemu/{riscof_inkemu.py,inkemu_isa.yaml,inkemu_platform.yaml,__init__.py}`
- Create: `tools/conformance/riscof/inkemu/env/{model_test.h,link.ld}`
- Create: `tools/conformance/run-arch-test.sh`
- Modify: `tools/conformance/versions.env`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `inkemu rvtest <ELF> --signature <FILE>` (Task 8).
- Produces: `tools/conformance/run-arch-test.sh` → code de sortie 0 si toutes les signatures RV32I, M, C, Zicsr et Zifencei concordent.

- [ ] **Step 1: Épingler les versions**

Run :
```bash
git ls-remote --tags https://github.com/riscv-non-isa/riscv-arch-test | grep -oE 'refs/tags/3\.[0-9]+\.[0-9]+$' | sort -V | tail -1
gh release list -R riscv/sail-riscv --limit 5
```
Ajouter à `versions.env` :
```bash
RISCV_ARCH_TEST_TAG=<tag 3.x trouvé>
SAIL_RISCV_RELEASE=<tag de release Sail le plus récent>
RISCOF_VERSION=<dernière version sur PyPI : pip index versions riscof>
```

- [ ] **Step 2: Image Docker**

`tools/docker/riscof/Dockerfile` :
```dockerfile
# Conformance tooling only (GPL/BSD tools): never linked into or shipped with inkemu.
FROM ubuntu:24.04
ARG RISCV_ARCH_TEST_TAG
ARG SAIL_RISCV_RELEASE
ARG RISCOF_VERSION
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-riscv64-unknown-elf binutils-riscv64-unknown-elf \
      make git ca-certificates curl python3 python3-venv \
 && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/venv && /opt/venv/bin/pip install --no-cache-dir "riscof==${RISCOF_VERSION}"
ENV PATH="/opt/venv/bin:/opt/sail/bin:${PATH}"
RUN git clone -q --depth 1 --branch "${RISCV_ARCH_TEST_TAG}" https://github.com/riscv-non-isa/riscv-arch-test /opt/riscv-arch-test
RUN mkdir -p /opt/sail \
 && curl -fsSL "https://github.com/riscv/sail-riscv/releases/download/${SAIL_RISCV_RELEASE}/sail-riscv-Linux-x86_64.tar.gz" \
    | tar -xz -C /opt/sail --strip-components=1
```
Si le nom de l'archive Sail diffère, le relever avec `gh release view -R riscv/sail-riscv "$SAIL_RISCV_RELEASE" --json assets -q '.assets[].name'` et corriger l'URL. Ensuite, lire `/opt/riscv-arch-test/riscof-plugins/rv32/sail_cSim/riscof_sail_cSim.py` pour connaître le **nom exact de l'exécutable** Sail qu'il invoque. S'il diffère du binaire fourni par la release, ajouter au Dockerfile `RUN ln -s /opt/sail/bin/<binaire fourni> /opt/sail/bin/<nom attendu>`.

- [ ] **Step 3: Environnement de test du DUT**

`env/model_test.h` (convention des plugins RISCOF ; `tohost = 1` termine le test, ce que `TestMachine` traite) :
```c
#ifndef _INKEMU_MODEL_TEST_H
#define _INKEMU_MODEL_TEST_H

#define RVMODEL_DATA_SECTION \
        .pushsection .tohost,"aw",@progbits;                \
        .align 8; .global tohost; tohost: .dword 0;         \
        .align 8; .global fromhost; fromhost: .dword 0;     \
        .popsection;                                        \
        .align 8; .global begin_regstate; begin_regstate:   \
        .word 128;                                          \
        .align 8; .global end_regstate; end_regstate:       \
        .word 4;

#define RVMODEL_HALT                                        \
  li x1, 1;                                                 \
  write_tohost:                                             \
    sw x1, tohost, t5;                                      \
    j write_tohost;

#define RVMODEL_BOOT
#define RVMODEL_DATA_BEGIN RVMODEL_DATA_SECTION .align 4; .global begin_signature; begin_signature:
#define RVMODEL_DATA_END .align 4; .global end_signature; end_signature:

#define RVMODEL_IO_INIT
#define RVMODEL_IO_WRITE_STR(_R, _STR)
#define RVMODEL_IO_CHECK()
#define RVMODEL_IO_ASSERT_GPR_EQ(_S, _R, _I)
#define RVMODEL_IO_ASSERT_SFPR_EQ(_F, _R, _I)
#define RVMODEL_IO_ASSERT_DFPR_EQ(_D, _R, _I)

#define RVMODEL_SET_MSW_INT
#define RVMODEL_CLEAR_MSW_INT
#define RVMODEL_CLEAR_MTIMER_INT
#define RVMODEL_CLEAR_MEXT_INT

#endif
```
`env/link.ld` :
```
OUTPUT_ARCH( "riscv" )
ENTRY(rvtest_entry_point)
SECTIONS
{
  . = 0x80000000;
  .text.init : { *(.text.init) }
  . = ALIGN(0x1000);
  .tohost : { *(.tohost) }
  . = ALIGN(0x1000);
  .text : { *(.text) }
  . = ALIGN(0x1000);
  .data : { *(.data) }
  .data.string : { *(.data.string) }
  .bss : { *(.bss) }
  _end = .;
}
```

- [ ] **Step 4: Description ISA et plateforme**

`inkemu_isa.yaml` (`misa` = `0x40001104` : RV32, I, M, C — même valeur que `MISA_RV32IMC` dans `csr.rs`) :
```yaml
hart_ids: [0]
hart0:
  ISA: RV32IMCZicsr_Zifencei
  physical_addr_sz: 32
  User_Spec_Version: '2.3'
  supported_xlen: [32]
  misa:
    reset-val: 0x40001104
    rv32:
      accessible: true
      mxl:
        implemented: true
        type:
          warl:
            dependency_fields: []
            legal: [mxl[1:0] in [0x1]]
            wr_illegal: [Unchanged]
      extensions:
        implemented: true
        type:
          warl:
            dependency_fields: []
            legal: [extensions[25:0] bitmask [0x0001104, 0x0000000]]
            wr_illegal: [Unchanged]
```
`inkemu_platform.yaml` :
```yaml
mtime:
  implemented: false
mtimecmp:
  implemented: false
nmi:
  label: nmi_vector
reset:
  label: reset_vector
```

- [ ] **Step 5: Plugin RISCOF**

`riscof_inkemu.py` (`__init__.py` vide à côté) :
```python
"""RISCOF DUT plugin: compiles each arch test and runs it on `inkemu rvtest`."""
import os

from riscof import utils
from riscof.pluginTemplate import pluginTemplate


class inkemu(pluginTemplate):
    __model__ = "inkemu"
    __version__ = "0.1.0"

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        config = kwargs.get("config")
        if config is None:
            raise SystemExit("inkemu plugin: missing [inkemu] section in config.ini")
        self.dut_exe = os.path.join(config.get("PATH", ""), "inkemu")
        self.num_jobs = str(config.get("jobs", 1))
        self.pluginpath = os.path.abspath(config["pluginpath"])
        self.isa_spec = os.path.abspath(config["ispec"])
        self.platform_spec = os.path.abspath(config["pspec"])
        self.target_run = config.get("target_run", "1") == "1"

    def initialise(self, suite, work_dir, archtest_env):
        self.work_dir = work_dir
        self.suite_dir = suite
        self.compile_cmd = (
            "riscv64-unknown-elf-gcc -march={march} -mabi=ilp32 -static -mcmodel=medany "
            "-fvisibility=hidden -nostdlib -nostartfiles -g "
            f"-T {self.pluginpath}/env/link.ld -I {self.pluginpath}/env/ -I {archtest_env} "
            "{macros} -o {elf} {test}"
        )

    def build(self, isa_yaml, platform_yaml):
        ispec = utils.load_yaml(isa_yaml)["hart0"]
        if 32 not in ispec["supported_xlen"]:
            raise SystemExit("inkemu plugin: only RV32 is supported")

    def runTests(self, testList):
        make = utils.makeUtil(makefilePath=os.path.join(self.work_dir, "Makefile." + self.name[:-1]))
        make.makeCommand = "make -k -j" + self.num_jobs
        for entry in testList.values():
            elf = "dut.elf"
            sig = os.path.join(entry["work_dir"], self.name[:-1] + ".signature")
            macros = " ".join("-D" + m for m in entry["macros"])
            # Test ISA strings look like "RV32IC" or "RV32I_Zicsr"; GCC needs zicsr/zifencei spelled out.
            base = entry["isa"].lower().split("_")[0].replace("zicsr", "").replace("zifencei", "")
            march = base + "_zicsr_zifencei"
            compile_cmd = self.compile_cmd.format(march=march, macros=macros, elf=elf, test=entry["test_path"])
            run_cmd = f"{self.dut_exe} rvtest {elf} --signature {sig}" if self.target_run else "true"
            make.add_target(f"@cd {entry['work_dir']}; {compile_cmd}; {run_cmd};")
        make.execute_all(self.work_dir)
        if not self.target_run:
            raise SystemExit(0)
```
`config.ini` (chemins vus depuis le conteneur, où le dépôt est monté sur `/work`) :
```ini
[RISCOF]
ReferencePlugin=sail_cSim
ReferencePluginPath=/opt/riscv-arch-test/riscof-plugins/rv32/sail_cSim
DUTPlugin=inkemu
DUTPluginPath=/work/tools/conformance/riscof/inkemu

[inkemu]
pluginpath=/work/tools/conformance/riscof/inkemu
ispec=/work/tools/conformance/riscof/inkemu/inkemu_isa.yaml
pspec=/work/tools/conformance/riscof/inkemu/inkemu_platform.yaml
target_run=1
jobs=4
PATH=/work/target/release

[sail_cSim]
pluginpath=/opt/riscv-arch-test/riscof-plugins/rv32/sail_cSim
jobs=4
```

- [ ] **Step 6: Script de lancement**

`tools/conformance/run-arch-test.sh` :
```bash
#!/usr/bin/env bash
# Builds inkemu and runs riscv-arch-test (RV32IMC + Zicsr + Zifencei) against Sail through RISCOF.
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
cargo build --release -p emu-cli
docker build -q -t inkemu/riscof \
  --build-arg RISCV_ARCH_TEST_TAG="$RISCV_ARCH_TEST_TAG" \
  --build-arg SAIL_RISCV_RELEASE="$SAIL_RISCV_RELEASE" \
  --build-arg RISCOF_VERSION="$RISCOF_VERSION" tools/docker/riscof
docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -v "$PWD:/work" -w /tmp inkemu/riscof bash -euo pipefail -c '
  riscof run --config=/work/tools/conformance/riscof/config.ini \
    --suite=/opt/riscv-arch-test/riscv-test-suite/ \
    --env=/opt/riscv-arch-test/riscv-test-suite/env \
    --work-dir=/tmp/riscof_work --no-browser | tee /tmp/riscof.log
  ! grep -E "\bFailed\b" /tmp/riscof.log'
```
Le dernier `grep` inverse fait échouer le script dès qu'une ligne de rapport contient `Failed`. Avec `set -o pipefail`, une erreur de `riscof` lui-même fait aussi échouer le script (voir la règle sur les codes de sortie dans les tubes). `chmod +x` le script. Le binaire `target/release/inkemu` est compilé sur l'hôte puis exécuté dans le conteneur Ubuntu 24.04 : les deux doivent avoir une glibc compatible, ce qui est le cas sur `ubuntu-24.04` en CI.

- [ ] **Step 7: Exécuter et corriger**

Run: `tools/conformance/run-arch-test.sh`
Expected: code de sortie 0, aucune ligne `Failed`.
Une signature divergente se diagnostique en comparant `/tmp/riscof_work/<test>/dut/*.signature` et `ref/*.signature`. On corrige toujours le cœur : la spec exige 100 % (§10.3, critère 1), cette suite n'a donc pas de liste d'exclusion.

- [ ] **Step 8: CI**

Ajouter au workflow un job `arch-test` (déclenché sur `push` vers `main` et sur les PR qui touchent `crates/cpu-rv32/**` ou `tools/conformance/**`) :
```yaml
  arch-test:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.99.0" }
      - uses: Swatinem/rust-cache@v2
      - run: tools/conformance/run-arch-test.sh
```

- [ ] **Step 9: Commit**

```bash
git add tools/docker/riscof tools/conformance .github/workflows/ci.yml
git commit -m "test(rv32): riscv-arch-test conformance through RISCOF against Sail" -- tools/docker/riscof tools/conformance .github/workflows/ci.yml
```

---

### Task 10: `firmware` — lecture et validation des images d'application ESP-IDF

Format de référence : ESP-IDF, *App Image Format* (`esp_image_header_t`, `esp_image_segment_header_t`, `esp_app_desc_t`) et `esptool` (`bin_image.py`). Disposition :

| Octets | Champ |
|---|---|
| 0 | magic `0xE9` |
| 1 | nombre de segments |
| 2 | mode SPI (0 QIO, 1 QOUT, 2 DIO, 3 DOUT, 4 FAST_READ, 5 SLOW_READ) |
| 3 | quartet bas : fréquence flash ; quartet haut : taille flash (0 = 1 Mo … 4 = 16 Mo … 7 = 128 Mo) |
| 4..8 | point d'entrée (LE) |
| 8 | `wp_pin` ; 9..12 `spi_pin_drv` |
| 12..14 | `chip_id` (LE) : 0 ESP32, 2 S2, 5 C3, 9 S3, 12 C2, 13 C6, 16 H2 |
| 14 | `min_chip_rev` ; 15..17 `min_chip_rev_full` ; 17..19 `max_chip_rev_full` ; 19..23 réservé |
| 23 | `hash_appended` |
| 24.. | segments : `load_addr` (u32), `data_len` (u32, multiple de 4), données |
| fin | remplissage jusqu'à ce que `position % 16 == 15`, octet de checksum (XOR des données de tous les segments, graine `0xEF`), puis SHA-256 de tout ce qui précède si `hash_appended` |

Le descripteur d'application (`esp_app_desc_t`, magic `0xABCD5432`) se trouve au début des données du premier segment.

Pour que les tests (de cette crate et du CLI) puissent fabriquer des images valides ou corrompues sans fixtures binaires, la crate expose un **constructeur d'images** derrière la feature `test-support`.

**Files:**
- Create: `crates/firmware/src/{error.rs,chip.rs,image.rs,app_desc.rs,builder.rs}`
- Modify: `crates/firmware/src/lib.rs`, `crates/firmware/Cargo.toml` (`[features] test-support = []`)

**Interfaces:**
- Produces :
  - `enum ChipId { Esp32, Esp32S2, Esp32C3, Esp32S3, Esp32C2, Esp32C6, Esp32H2, Unknown(u16) }` + `from_raw(u16)`, `name(&self) -> String`.
  - `enum FlashSize { Mb1, Mb2, Mb4, Mb8, Mb16, Mb32, Mb64, Mb128 }` + `from_nibble(u8) -> Option<FlashSize>`, `bytes(self) -> u64`, `megabytes(self) -> u32`.
  - `enum SpiMode { Qio, Qout, Dio, Dout, FastRead, SlowRead }` + `from_raw(u8) -> Option<SpiMode>`.
  - `struct Segment { load_addr: u32, data: Vec<u8>, file_offset: usize }`.
  - `struct AppDesc { secure_version: u32, version: String, project_name: String, time: String, date: String, idf_ver: String, elf_sha256: [u8; 32] }`.
  - `struct FirmwareImage { chip: ChipId, entry: u32, spi_mode: SpiMode, flash_size: FlashSize, flash_freq_raw: u8, min_chip_rev_full: u16, max_chip_rev_full: u16, segments: Vec<Segment>, hash_appended: bool, app_desc: Option<AppDesc> }`.
  - `FirmwareImage::parse(bytes: &[u8]) -> Result<FirmwareImage, FirmwareError>`.
  - `fn require_chip(&self, expected: ChipId) -> Result<(), FirmwareError>` ; `fn require_flash(&self, available: FlashSize) -> Result<(), FirmwareError>`.
  - (feature `test-support`) `struct ImageSpec { chip: u16, entry: u32, flash_size_nibble: u8, spi_mode: u8, segments: Vec<(u32, Vec<u8>)>, hash_appended: bool }` + `Default` ; `fn build_image(&ImageSpec) -> Vec<u8>` ; `fn app_desc_bytes(version: &str, project: &str, idf: &str) -> Vec<u8>`.

- [ ] **Step 1: Erreurs**

`error.rs` :
```rust
//! Every message states the cause and what the user can do about it.
use crate::chip::{ChipId, FlashSize};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FirmwareError {
    #[error("the file is empty or shorter than an image header ({len} bytes); select an ESP-IDF application .bin")]
    TooShort { len: usize },
    #[error("this is an encrypted OTA update (.xota); inkemu needs the plain application image — use a flash dump or a plain .bin release")]
    EncryptedOta,
    #[error("not an ESP-IDF application image: first byte is {found:#04x}, expected 0xe9; a merged flash image or another format was probably selected")]
    BadMagic { found: u8 },
    #[error("image declares {count} segments, more than the {max} an ESP-IDF image can have; the file is corrupted")]
    TooManySegments { count: u8, max: u8 },
    #[error("segment {index} at file offset {offset} declares {declared} bytes but only {available} remain; the file is truncated or corrupted")]
    TruncatedSegment { index: usize, offset: usize, declared: u64, available: usize },
    #[error("segment {index} length {len} is not a multiple of 4; the file is corrupted")]
    MisalignedSegment { index: usize, len: u32 },
    #[error("unknown SPI flash mode {raw}; the header is corrupted")]
    BadSpiMode { raw: u8 },
    #[error("unknown flash size code {raw}; the header is corrupted")]
    BadFlashSize { raw: u8 },
    #[error("the image ends before its checksum/hash trailer; the file is truncated")]
    TruncatedTrailer,
    #[error("checksum mismatch (stored {stored:#04x}, computed {computed:#04x}); the file is corrupted")]
    ChecksumMismatch { stored: u8, computed: u8 },
    #[error("SHA-256 mismatch; the file is corrupted or was modified after signing")]
    HashMismatch,
    #[error("this firmware is built for {found}, but the selected device uses {expected}; choose the matching device or firmware")]
    ChipMismatch { expected: ChipId, found: ChipId },
    #[error("this firmware expects {needed} MB of flash, but the selected device has {available} MB")]
    FlashTooSmall { needed: u32, available: u32 },
}
```
`ChipId` implémente `Display` via `name()` (pour les messages ci-dessus).

- [ ] **Step 2: Types de puce**

`chip.rs` :
```rust
//! Header enumerations of ESP-IDF images (values from esptool / esp_app_format.h).
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChipId { Esp32, Esp32S2, Esp32C3, Esp32S3, Esp32C2, Esp32C6, Esp32H2, Unknown(u16) }

impl ChipId {
    #[must_use]
    pub const fn from_raw(raw: u16) -> ChipId {
        match raw { 0 => ChipId::Esp32, 2 => ChipId::Esp32S2, 5 => ChipId::Esp32C3, 9 => ChipId::Esp32S3,
                    12 => ChipId::Esp32C2, 13 => ChipId::Esp32C6, 16 => ChipId::Esp32H2, other => ChipId::Unknown(other) }
    }
    #[must_use]
    pub fn name(&self) -> String {
        match self {
            ChipId::Esp32 => "ESP32".into(), ChipId::Esp32S2 => "ESP32-S2".into(), ChipId::Esp32C3 => "ESP32-C3".into(),
            ChipId::Esp32S3 => "ESP32-S3".into(), ChipId::Esp32C2 => "ESP32-C2".into(), ChipId::Esp32C6 => "ESP32-C6".into(),
            ChipId::Esp32H2 => "ESP32-H2".into(), ChipId::Unknown(id) => format!("unknown chip (id {id})"),
        }
    }
}

impl fmt::Display for ChipId { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.name()) } }

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FlashSize { Mb1, Mb2, Mb4, Mb8, Mb16, Mb32, Mb64, Mb128 }

impl FlashSize {
    #[must_use]
    pub const fn from_nibble(n: u8) -> Option<FlashSize> {
        Some(match n { 0 => FlashSize::Mb1, 1 => FlashSize::Mb2, 2 => FlashSize::Mb4, 3 => FlashSize::Mb8,
                       4 => FlashSize::Mb16, 5 => FlashSize::Mb32, 6 => FlashSize::Mb64, 7 => FlashSize::Mb128, _ => return None })
    }
    #[must_use]
    pub const fn megabytes(self) -> u32 { 1 << (self as u32) }
    #[must_use]
    pub const fn bytes(self) -> u64 { (self.megabytes() as u64) * 1024 * 1024 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpiMode { Qio, Qout, Dio, Dout, FastRead, SlowRead }

impl SpiMode {
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<SpiMode> {
        Some(match raw { 0 => SpiMode::Qio, 1 => SpiMode::Qout, 2 => SpiMode::Dio, 3 => SpiMode::Dout,
                         4 => SpiMode::FastRead, 5 => SpiMode::SlowRead, _ => return None })
    }
}
```

- [ ] **Step 3: Constructeur de test**

`builder.rs` (module `#[cfg(any(test, feature = "test-support"))] pub mod builder;` dans `lib.rs`) :
```rust
//! Builds valid ESP-IDF application images for tests (feature `test-support`).
#![allow(clippy::cast_possible_truncation)]
use crate::image::{CHECKSUM_SEED, IMAGE_MAGIC, TRAILER_ALIGN};
use crate::app_desc::APP_DESC_MAGIC;
use sha2::{Digest, Sha256};

/// Write-protect pin value meaning "disabled" in ESP-IDF headers.
const WP_PIN_DISABLED: u8 = 0xEE;
const FLASH_FREQ_80M: u8 = 0xF;

#[derive(Debug, Clone)]
pub struct ImageSpec { pub chip: u16, pub entry: u32, pub flash_size_nibble: u8, pub spi_mode: u8, pub segments: Vec<(u32, Vec<u8>)>, pub hash_appended: bool }

impl Default for ImageSpec {
    fn default() -> Self {
        ImageSpec { chip: 5, entry: 0x4038_086E, flash_size_nibble: 4, spi_mode: 2,
                    segments: vec![(0x3C00_0020, app_desc_bytes("1.0.0", "demo", "v5.5.2")), (0x4038_0000, vec![0x13, 0, 0, 0])],
                    hash_appended: true }
    }
}

#[must_use]
pub fn build_image(spec: &ImageSpec) -> Vec<u8> {
    let mut out = vec![IMAGE_MAGIC, spec.segments.len() as u8, spec.spi_mode, (spec.flash_size_nibble << 4) | FLASH_FREQ_80M];
    out.extend(spec.entry.to_le_bytes());
    out.extend([WP_PIN_DISABLED, 0, 0, 0]);
    out.extend(spec.chip.to_le_bytes());
    out.push(0);                                   // min_chip_rev (legacy)
    out.extend(0u16.to_le_bytes());                // min_chip_rev_full
    out.extend(0xFFFFu16.to_le_bytes());           // max_chip_rev_full
    out.extend([0u8; 4]);                          // reserved
    out.push(u8::from(spec.hash_appended));
    let mut checksum = CHECKSUM_SEED;
    for (addr, data) in &spec.segments {
        out.extend(addr.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        checksum = data.iter().fold(checksum, |c, b| c ^ b);
    }
    while out.len() % TRAILER_ALIGN != TRAILER_ALIGN - 1 { out.push(0); }
    out.push(checksum);
    if spec.hash_appended { let digest = Sha256::digest(&out); out.extend(digest); }
    out
}

/// A 256-byte `esp_app_desc_t` with the given strings (truncated to their field sizes).
#[must_use]
pub fn app_desc_bytes(version: &str, project: &str, idf: &str) -> Vec<u8> {
    let mut d = vec![0u8; 256];
    d[0..4].copy_from_slice(&APP_DESC_MAGIC.to_le_bytes());
    let put = |d: &mut Vec<u8>, at: usize, len: usize, s: &str| { let b = s.as_bytes(); let n = b.len().min(len - 1); d[at..at + n].copy_from_slice(&b[..n]); };
    put(&mut d, 16, 32, version);
    put(&mut d, 48, 32, project);
    put(&mut d, 80, 16, "12:00:00");
    put(&mut d, 96, 16, "Oct 11 2026");
    put(&mut d, 112, 32, idf);
    d
}
```

- [ ] **Step 4: Tests (échouent)**

En bas de `image.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{build_image, ImageSpec};

    fn with<F: FnOnce(&mut Vec<u8>)>(f: F) -> Vec<u8> { let mut b = build_image(&ImageSpec::default()); f(&mut b); b }

    #[test]
    fn parses_valid_image() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec::default())).unwrap();
        assert_eq!(img.chip, ChipId::Esp32C3);
        assert_eq!(img.entry, 0x4038_086E);
        assert_eq!((img.spi_mode, img.flash_size), (SpiMode::Dio, FlashSize::Mb16));
        assert_eq!(img.segments.len(), 2);
        assert_eq!(img.segments[1].load_addr, 0x4038_0000);
        let desc = img.app_desc.unwrap();
        assert_eq!((desc.version.as_str(), desc.project_name.as_str(), desc.idf_ver.as_str()), ("1.0.0", "demo", "v5.5.2"));
    }

    #[test]
    fn accepts_image_without_hash() {
        let spec = ImageSpec { hash_appended: false, ..ImageSpec::default() };
        assert!(FirmwareImage::parse(&build_image(&spec)).is_ok());
    }

    #[test]
    fn rejects_empty_and_short_files() {
        assert_eq!(FirmwareImage::parse(&[]), Err(FirmwareError::TooShort { len: 0 }));
        assert_eq!(FirmwareImage::parse(&[0xE9; 10]), Err(FirmwareError::TooShort { len: 10 }));
    }

    #[test]
    fn rejects_encrypted_xota() {
        let mut b = b"XOTA".to_vec(); b.resize(512, 0);
        assert_eq!(FirmwareImage::parse(&b), Err(FirmwareError::EncryptedOta));
    }

    #[test]
    fn rejects_bad_magic() {
        assert_eq!(FirmwareImage::parse(&with(|b| b[0] = 0x00)), Err(FirmwareError::BadMagic { found: 0 }));
    }

    #[test]
    fn rejects_too_many_segments() {
        assert_eq!(FirmwareImage::parse(&with(|b| b[1] = 200)), Err(FirmwareError::TooManySegments { count: 200, max: 16 }));
    }

    #[test]
    fn rejects_truncated_segment() {
        let full = build_image(&ImageSpec::default());
        let cut = &full[..40];                                    // inside the first segment's data
        assert!(matches!(FirmwareImage::parse(cut), Err(FirmwareError::TruncatedSegment { index: 0, offset: 24, .. })));
    }

    #[test]
    fn rejects_absurd_segment_length() {
        // data_len of segment 0 lives at bytes 28..32
        let b = with(|b| b[28..32].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes()));
        assert!(matches!(FirmwareImage::parse(&b), Err(FirmwareError::TruncatedSegment { index: 0, declared: 0xFFFF_FFF0, .. })));
    }

    #[test]
    fn rejects_misaligned_segment_length() {
        let spec = ImageSpec { segments: vec![(0x4038_0000, vec![1, 2, 3])], ..ImageSpec::default() };
        assert_eq!(FirmwareImage::parse(&build_image(&spec)), Err(FirmwareError::MisalignedSegment { index: 0, len: 3 }));
    }

    #[test]
    fn rejects_bad_checksum_and_bad_hash() {
        let spec = ImageSpec { hash_appended: false, ..ImageSpec::default() };
        let mut b = build_image(&spec);
        let last = b.len() - 1;
        b[last] ^= 0xFF;
        assert!(matches!(FirmwareImage::parse(&b), Err(FirmwareError::ChecksumMismatch { .. })));
        let mut b = build_image(&ImageSpec::default());
        let last = b.len() - 1;
        b[last] ^= 0xFF;                                          // flips a byte of the SHA-256
        assert_eq!(FirmwareImage::parse(&b), Err(FirmwareError::HashMismatch));
    }

    #[test]
    fn rejects_truncated_trailer() {
        let full = build_image(&ImageSpec::default());
        assert_eq!(FirmwareImage::parse(&full[..full.len() - 5]), Err(FirmwareError::TruncatedTrailer));
    }

    #[test]
    fn unknown_chip_is_kept_and_reported() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec { chip: 99, ..ImageSpec::default() })).unwrap();
        assert_eq!(img.chip, ChipId::Unknown(99));
    }

    #[test]
    fn chip_mismatch_names_both_chips() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec { chip: 9, ..ImageSpec::default() })).unwrap();
        let err = img.require_chip(ChipId::Esp32C3).unwrap_err();
        assert_eq!(err, FirmwareError::ChipMismatch { expected: ChipId::Esp32C3, found: ChipId::Esp32S3 });
        let msg = err.to_string();
        assert!(msg.contains("ESP32-S3") && msg.contains("ESP32-C3"), "{msg}");
    }

    #[test]
    fn flash_size_must_fit_device() {
        let img = FirmwareImage::parse(&build_image(&ImageSpec::default())).unwrap();
        assert_eq!(img.require_flash(FlashSize::Mb4), Err(FirmwareError::FlashTooSmall { needed: 16, available: 4 }));
        assert_eq!(img.require_flash(FlashSize::Mb16), Ok(()));
    }

    #[test]
    fn image_without_app_desc_parses_with_none() {
        let spec = ImageSpec { segments: vec![(0x4038_0000, vec![0; 8])], ..ImageSpec::default() };
        assert!(FirmwareImage::parse(&build_image(&spec)).unwrap().app_desc.is_none());
    }
}
```

- [ ] **Step 5: Constater l'échec** — Run: `cargo test -p firmware` → erreurs de compilation.

- [ ] **Step 6: Implémenter `app_desc.rs`**

```rust
//! `esp_app_desc_t`, found at the start of the first segment (ESP-IDF esp_app_format.h).
pub const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const LEN: usize = 256;
const SECURE_VERSION: usize = 4;
const VERSION: (usize, usize) = (16, 32);
const PROJECT_NAME: (usize, usize) = (48, 32);
const TIME: (usize, usize) = (80, 16);
const DATE: (usize, usize) = (96, 16);
const IDF_VER: (usize, usize) = (112, 32);
const ELF_SHA256: usize = 144;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDesc { pub secure_version: u32, pub version: String, pub project_name: String, pub time: String, pub date: String, pub idf_ver: String, pub elf_sha256: [u8; 32] }

fn c_string(bytes: &[u8], (at, len): (usize, usize)) -> String {
    let field = &bytes[at..at + len];
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn le_u32(b: &[u8], at: usize) -> u32 { u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) }

impl AppDesc {
    /// Returns `None` when the segment does not start with an app descriptor.
    #[must_use]
    pub fn parse(segment: &[u8]) -> Option<AppDesc> {
        if segment.len() < LEN || le_u32(segment, 0) != APP_DESC_MAGIC { return None; }
        let mut elf_sha256 = [0u8; 32];
        elf_sha256.copy_from_slice(&segment[ELF_SHA256..ELF_SHA256 + 32]);
        Some(AppDesc {
            secure_version: le_u32(segment, SECURE_VERSION),
            version: c_string(segment, VERSION), project_name: c_string(segment, PROJECT_NAME),
            time: c_string(segment, TIME), date: c_string(segment, DATE), idf_ver: c_string(segment, IDF_VER), elf_sha256,
        })
    }
}
```

- [ ] **Step 7: Implémenter `image.rs`**

```rust
//! ESP-IDF application image parsing and validation (spec §5.1).
use crate::app_desc::AppDesc;
use crate::chip::{ChipId, FlashSize, SpiMode};
use crate::error::FirmwareError;
use sha2::{Digest, Sha256};

pub const IMAGE_MAGIC: u8 = 0xE9;
const XOTA_MAGIC: &[u8; 4] = b"XOTA";
/// Common header (8 bytes) + extended header (16 bytes).
pub const HEADER_LEN: usize = 24;
const SEGMENT_HEADER_LEN: usize = 8;
/// ESP_IMAGE_MAX_SEGMENTS in ESP-IDF.
pub const MAX_SEGMENTS: u8 = 16;
pub const CHECKSUM_SEED: u8 = 0xEF;
/// The checksum byte sits at the last byte of a 16-byte block.
pub const TRAILER_ALIGN: usize = 16;
const SHA256_LEN: usize = 32;
const SEGMENT_ALIGN: u32 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment { pub load_addr: u32, pub data: Vec<u8>, pub file_offset: usize }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareImage {
    pub chip: ChipId, pub entry: u32, pub spi_mode: SpiMode, pub flash_size: FlashSize, pub flash_freq_raw: u8,
    pub min_chip_rev_full: u16, pub max_chip_rev_full: u16, pub segments: Vec<Segment>, pub hash_appended: bool, pub app_desc: Option<AppDesc>,
}

fn le_u16(b: &[u8], at: usize) -> u16 { u16::from_le_bytes([b[at], b[at + 1]]) }
fn le_u32(b: &[u8], at: usize) -> u32 { u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) }

impl FirmwareImage {
    /// # Errors
    /// Any [`FirmwareError`] describing why the file is not a usable application image.
    pub fn parse(bytes: &[u8]) -> Result<FirmwareImage, FirmwareError> {
        if bytes.starts_with(XOTA_MAGIC) { return Err(FirmwareError::EncryptedOta); }
        if bytes.len() < HEADER_LEN { return Err(FirmwareError::TooShort { len: bytes.len() }); }
        if bytes[0] != IMAGE_MAGIC { return Err(FirmwareError::BadMagic { found: bytes[0] }); }
        let count = bytes[1];
        if count > MAX_SEGMENTS { return Err(FirmwareError::TooManySegments { count, max: MAX_SEGMENTS }); }
        let spi_mode = SpiMode::from_raw(bytes[2]).ok_or(FirmwareError::BadSpiMode { raw: bytes[2] })?;
        let flash_size = FlashSize::from_nibble(bytes[3] >> 4).ok_or(FirmwareError::BadFlashSize { raw: bytes[3] >> 4 })?;
        let (segments, checksum, end) = Self::read_segments(bytes, usize::from(count))?;
        let hash_appended = bytes[23] == 1;
        Self::verify_trailer(bytes, end, checksum, hash_appended)?;
        let app_desc = segments.first().and_then(|s| AppDesc::parse(&s.data));
        Ok(FirmwareImage {
            chip: ChipId::from_raw(le_u16(bytes, 12)), entry: le_u32(bytes, 4), spi_mode, flash_size, flash_freq_raw: bytes[3] & 0x0F,
            min_chip_rev_full: le_u16(bytes, 15), max_chip_rev_full: le_u16(bytes, 17), segments, hash_appended, app_desc,
        })
    }

    fn read_segments(bytes: &[u8], count: usize) -> Result<(Vec<Segment>, u8, usize), FirmwareError> {
        let mut segments = Vec::with_capacity(count);
        let mut checksum = CHECKSUM_SEED;
        let mut pos = HEADER_LEN;
        for index in 0..count {
            let available = bytes.len().saturating_sub(pos + SEGMENT_HEADER_LEN);
            if bytes.len() < pos + SEGMENT_HEADER_LEN {
                return Err(FirmwareError::TruncatedSegment { index, offset: pos, declared: 0, available });
            }
            let load_addr = le_u32(bytes, pos);
            let len = le_u32(bytes, pos + 4);
            if u64::from(len) > available as u64 {
                return Err(FirmwareError::TruncatedSegment { index, offset: pos, declared: u64::from(len), available });
            }
            if len % SEGMENT_ALIGN != 0 { return Err(FirmwareError::MisalignedSegment { index, len }); }
            let start = pos + SEGMENT_HEADER_LEN;
            let data = bytes[start..start + len as usize].to_vec();
            checksum = data.iter().fold(checksum, |c, b| c ^ b);
            segments.push(Segment { load_addr, data, file_offset: start });
            pos = start + len as usize;
        }
        Ok((segments, checksum, pos))
    }

    fn verify_trailer(bytes: &[u8], end: usize, checksum: u8, hash_appended: bool) -> Result<(), FirmwareError> {
        let checksum_pos = end + (TRAILER_ALIGN - 1 - end % TRAILER_ALIGN);
        let stored = *bytes.get(checksum_pos).ok_or(FirmwareError::TruncatedTrailer)?;
        if stored != checksum { return Err(FirmwareError::ChecksumMismatch { stored, computed: checksum }); }
        if hash_appended {
            let hashed_len = checksum_pos + 1;
            let stored_hash = bytes.get(hashed_len..hashed_len + SHA256_LEN).ok_or(FirmwareError::TruncatedTrailer)?;
            if Sha256::digest(&bytes[..hashed_len]).as_slice() != stored_hash { return Err(FirmwareError::HashMismatch); }
        }
        Ok(())
    }

    /// # Errors
    /// [`FirmwareError::ChipMismatch`] when the image targets another SoC.
    pub fn require_chip(&self, expected: ChipId) -> Result<(), FirmwareError> {
        if self.chip == expected { Ok(()) } else { Err(FirmwareError::ChipMismatch { expected, found: self.chip }) }
    }

    /// # Errors
    /// [`FirmwareError::FlashTooSmall`] when the image header requires more flash than the device has.
    pub fn require_flash(&self, available: FlashSize) -> Result<(), FirmwareError> {
        if self.flash_size <= available { Ok(()) }
        else { Err(FirmwareError::FlashTooSmall { needed: self.flash_size.megabytes(), available: available.megabytes() }) }
    }
}
```
Les conversions `len as usize` sont sûres : `len` a été comparé à `available`, qui est un `usize`. Ajouter `#[allow(clippy::cast_possible_truncation)]` sur `read_segments` avec un commentaire `// INVARIANT:` qui le dit.

`lib.rs` :
```rust
//! Firmware image handling: ESP-IDF application images and partition tables.
#![cfg_attr(test, allow(clippy::unwrap_used))]
pub mod app_desc;
pub mod chip;
pub mod error;
pub mod image;
#[cfg(any(test, feature = "test-support"))]
pub mod builder;

pub use app_desc::AppDesc;
pub use chip::{ChipId, FlashSize, SpiMode};
pub use error::FirmwareError;
pub use image::{FirmwareImage, Segment};
```

- [ ] **Step 8: Lancer les tests et la couverture**

Run: `cargo test -p firmware && cargo llvm-cov -p firmware --summary-only`
Expected: PASS ; `image.rs` et `app_desc.rs` à 100 % de lignes (exigence de la spec sur le parsing d'images).

- [ ] **Step 9: Commit**

```bash
git commit -m "feat(firmware): parse and validate ESP-IDF application images" -- crates/firmware
```

---

### Task 11: `firmware` — tables de partitions (génération et lecture)

Format ESP-IDF (`gen_esp32part.py`) :
- entrées de 32 octets : magic `0x50AA` (octets `AA 50`), type, sous-type, `offset` (u32 LE), `size` (u32 LE), nom sur 16 octets complétés de `0`, `flags` (u32 LE ; bit 0 chiffré, bit 1 lecture seule) ;
- puis une entrée MD5 : `EB EB`, 14 octets `FF`, MD5 des entrées précédentes (16 octets) ;
- puis du remplissage `0xFF` jusqu'à `0xC00` octets. La table est écrite à `0x8000`.

Types : `app` = 0x00 (sous-types `factory` 0x00, `ota_0..15` 0x10..0x1F, `test` 0x20) ; `data` = 0x01 (`ota` 0x00, `phy` 0x01, `nvs` 0x02, `coredump` 0x03, `nvs_keys` 0x04, `efuse` 0x05, `spiffs` 0x82, `littlefs` 0x83, `fat` 0x81). Alignement : 0x10000 pour `app`, 0x1000 pour `data`.

La table de référence est la **disposition stock du X4** (recherche matérielle §3.2). Le fichier `.bin` de référence est produit par l'outil officiel dans Docker et committé : le test compare nos octets aux siens.

**Files:**
- Create: `crates/firmware/src/partition.rs`
- Modify: `crates/firmware/src/lib.rs`, `crates/firmware/Cargo.toml` (`md-5`)
- Create: `tools/docker/esp-idf/Dockerfile`, `tools/fixtures/build-partition-fixtures.sh`
- Create: `tests/fixtures/partitions/x4-stock.csv`, `tests/fixtures/partitions/x4-stock.bin` (généré)
- Modify: `tools/conformance/versions.env` (`ESP_IDF_VERSION`)

**Interfaces:**
- Produces :
  - `PARTITION_TABLE_OFFSET: u32 = 0x8000`, `PARTITION_TABLE_LEN: usize = 0xC00`.
  - `struct Partition { name: String, ptype: u8, subtype: u8, offset: u32, size: u32, flags: u32 }` + constantes `TYPE_APP, TYPE_DATA, SUBTYPE_FACTORY, SUBTYPE_OTA_0, SUBTYPE_OTA_1, SUBTYPE_DATA_OTA, SUBTYPE_NVS, SUBTYPE_COREDUMP, SUBTYPE_SPIFFS`.
  - `struct PartitionTable { entries: Vec<Partition> }` ; `fn to_bytes(&self, flash: FlashSize) -> Result<Vec<u8>, PartitionError>` ; `PartitionTable::parse(bytes: &[u8]) -> Result<PartitionTable, PartitionError>` ; `fn xteink_x4_stock() -> PartitionTable`.
  - `enum PartitionError { Empty, TooManyEntries { count }, NameTooLong { name }, Misaligned { name, offset, required }, BeyondFlash { name, end, flash_bytes }, Overlap { first, second }, BadEntryMagic { index }, Md5Mismatch }`.

- [ ] **Step 1: Fixture de référence**

`tests/fixtures/partitions/x4-stock.csv` :
```
# Name,   Type, SubType,  Offset,   Size,     Flags
nvs,      data, nvs,      0x9000,   0x5000,
otadata,  data, ota,      0xe000,   0x2000,
app0,     app,  ota_0,    0x10000,  0x640000,
app1,     app,  ota_1,    0x650000, 0x640000,
spiffs,   data, spiffs,   0xc90000, 0x360000,
coredump, data, coredump, 0xff0000, 0x10000,
```
Épingler la version : `docker manifest inspect espressif/idf:v5.5.2 >/dev/null && echo ok`. Si cette étiquette n'existe pas, prendre la dernière `v5.5.x` listée par `curl -s 'https://hub.docker.com/v2/repositories/espressif/idf/tags?page_size=100' | python3 -c "import sys,json;print('\n'.join(t['name'] for t in json.load(sys.stdin)['results'] if t['name'].startswith('v5.5')))"`. Puis ajouter à `tools/conformance/versions.env` : `ESP_IDF_VERSION=v5.5.2` (ou la version retenue).

`tools/docker/esp-idf/Dockerfile` :
```dockerfile
# ESP-IDF tooling for test fixtures only.
ARG ESP_IDF_VERSION
FROM espressif/idf:${ESP_IDF_VERSION}
```
`tools/fixtures/build-partition-fixtures.sh` :
```bash
#!/usr/bin/env bash
# Regenerates partition-table reference binaries with ESP-IDF's own gen_esp32part.py.
set -euo pipefail
cd "$(dirname "$0")/../.."
source tools/conformance/versions.env
docker build -q -t inkemu/esp-idf --build-arg ESP_IDF_VERSION="$ESP_IDF_VERSION" tools/docker/esp-idf
for csv in tests/fixtures/partitions/*.csv; do
  docker run --rm -u "$(id -u):$(id -g)" -v "$PWD:/w" inkemu/esp-idf \
    python /opt/esp/idf/components/partition_table/gen_esp32part.py --flash-size 16MB "/w/$csv" "/w/${csv%.csv}.bin"
done
```
Run: `chmod +x tools/fixtures/build-partition-fixtures.sh && tools/fixtures/build-partition-fixtures.sh && xxd tests/fixtures/partitions/x4-stock.bin | head -8`
Expected: la première ligne commence par `aa50 0102 0090 0000 0050 0000 6e76 7300` (entrée `nvs`). Si le script officiel n'est pas à ce chemin dans l'image, le trouver avec `docker run --rm inkemu/esp-idf find / -name gen_esp32part.py 2>/dev/null`.

- [ ] **Step 2: Tests (échouent)**

En bas de `partition.rs` :
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::chip::FlashSize;

    fn golden() -> Vec<u8> { std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/partitions/x4-stock.bin")).unwrap() }

    #[test]
    fn x4_stock_matches_esp_idf_byte_for_byte() {
        assert_eq!(xteink_x4_stock().to_bytes(FlashSize::Mb16).unwrap(), golden());
    }

    #[test]
    fn parses_esp_idf_output() {
        let t = PartitionTable::parse(&golden()).unwrap();
        assert_eq!(t, xteink_x4_stock());
        assert_eq!(t.entries[2].name, "app0");
        assert_eq!((t.entries[2].ptype, t.entries[2].subtype), (TYPE_APP, SUBTYPE_OTA_0));
    }

    #[test]
    fn detects_corrupted_md5() {
        let mut b = golden();
        b[40] ^= 1;                                               // inside entry 1
        assert_eq!(PartitionTable::parse(&b), Err(PartitionError::Md5Mismatch));
    }

    fn table(entries: Vec<(&str, u8, u32, u32)>) -> PartitionTable {
        PartitionTable { entries: entries.into_iter().map(|(n, t, o, s)| Partition { name: n.into(), ptype: t, subtype: 0, offset: o, size: s, flags: 0 }).collect() }
    }

    #[test]
    fn rejects_invalid_layouts() {
        assert_eq!(PartitionTable { entries: vec![] }.to_bytes(FlashSize::Mb4), Err(PartitionError::Empty));
        assert!(matches!(table(vec![("app", TYPE_APP, 0x11000, 0x10000)]).to_bytes(FlashSize::Mb4),
                         Err(PartitionError::Misaligned { required: 0x10000, .. })));
        assert!(matches!(table(vec![("a", TYPE_DATA, 0x9000, 0x2000), ("b", TYPE_DATA, 0xA000, 0x1000)]).to_bytes(FlashSize::Mb4),
                         Err(PartitionError::Overlap { .. })));
        assert!(matches!(table(vec![("big", TYPE_APP, 0x10000, 0x40_0000)]).to_bytes(FlashSize::Mb4),
                         Err(PartitionError::BeyondFlash { .. })));
        assert!(matches!(table(vec![("a_name_far_too_long", TYPE_DATA, 0x9000, 0x1000)]).to_bytes(FlashSize::Mb4),
                         Err(PartitionError::NameTooLong { .. })));
    }

    #[test]
    fn rejects_garbage_entry() {
        let mut b = golden();
        b[32] = 0x12;                                              // magic of entry 1
        assert_eq!(PartitionTable::parse(&b), Err(PartitionError::BadEntryMagic { index: 1 }));
    }
}
```

- [ ] **Step 3: Constater l'échec** — Run: `cargo test -p firmware partition` → erreurs de compilation.

- [ ] **Step 4: Implémenter**

```rust
//! ESP-IDF partition tables, bit-compatible with gen_esp32part.py.
use crate::chip::FlashSize;
use md5::{Digest, Md5};
use thiserror::Error;

pub const PARTITION_TABLE_OFFSET: u32 = 0x8000;
pub const PARTITION_TABLE_LEN: usize = 0xC00;
const ENTRY_LEN: usize = 32;
const ENTRY_MAGIC: [u8; 2] = [0xAA, 0x50];
const MD5_MAGIC: [u8; 2] = [0xEB, 0xEB];
const ERASED: u8 = 0xFF;
const NAME_LEN: usize = 16;
/// One slot is reserved for the MD5 entry.
const MAX_ENTRIES: usize = PARTITION_TABLE_LEN / ENTRY_LEN - 1;
const APP_ALIGN: u32 = 0x10000;
const DATA_ALIGN: u32 = 0x1000;

pub const TYPE_APP: u8 = 0x00;
pub const TYPE_DATA: u8 = 0x01;
pub const SUBTYPE_FACTORY: u8 = 0x00;
pub const SUBTYPE_OTA_0: u8 = 0x10;
pub const SUBTYPE_OTA_1: u8 = 0x11;
pub const SUBTYPE_DATA_OTA: u8 = 0x00;
pub const SUBTYPE_NVS: u8 = 0x02;
pub const SUBTYPE_COREDUMP: u8 = 0x03;
pub const SUBTYPE_SPIFFS: u8 = 0x82;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition { pub name: String, pub ptype: u8, pub subtype: u8, pub offset: u32, pub size: u32, pub flags: u32 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartitionTable { pub entries: Vec<Partition> }

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PartitionError {
    #[error("the partition table is empty")]
    Empty,
    #[error("{count} partitions exceed the maximum of {MAX_ENTRIES}")]
    TooManyEntries { count: usize },
    #[error("partition name `{name}` is longer than 15 characters")]
    NameTooLong { name: String },
    #[error("partition `{name}` starts at {offset:#x}, which is not aligned to {required:#x}")]
    Misaligned { name: String, offset: u32, required: u32 },
    #[error("partition `{name}` ends at {end:#x}, beyond the {flash_bytes:#x}-byte flash")]
    BeyondFlash { name: String, end: u64, flash_bytes: u64 },
    #[error("partitions `{first}` and `{second}` overlap")]
    Overlap { first: String, second: String },
    #[error("entry {index} of the partition table is not a valid entry")]
    BadEntryMagic { index: usize },
    #[error("the partition table MD5 does not match its entries; the table is corrupted")]
    Md5Mismatch,
}

impl PartitionTable {
    fn validate(&self, flash: FlashSize) -> Result<(), PartitionError> {
        if self.entries.is_empty() { return Err(PartitionError::Empty); }
        if self.entries.len() > MAX_ENTRIES { return Err(PartitionError::TooManyEntries { count: self.entries.len() }); }
        for p in &self.entries {
            if p.name.len() >= NAME_LEN || !p.name.is_ascii() { return Err(PartitionError::NameTooLong { name: p.name.clone() }); }
            let required = if p.ptype == TYPE_APP { APP_ALIGN } else { DATA_ALIGN };
            if p.offset % required != 0 { return Err(PartitionError::Misaligned { name: p.name.clone(), offset: p.offset, required }); }
            let end = u64::from(p.offset) + u64::from(p.size);
            if end > flash.bytes() { return Err(PartitionError::BeyondFlash { name: p.name.clone(), end, flash_bytes: flash.bytes() }); }
        }
        let mut sorted: Vec<&Partition> = self.entries.iter().collect();
        sorted.sort_by_key(|p| p.offset);
        for pair in sorted.windows(2) {
            if u64::from(pair[0].offset) + u64::from(pair[0].size) > u64::from(pair[1].offset) {
                return Err(PartitionError::Overlap { first: pair[0].name.clone(), second: pair[1].name.clone() });
            }
        }
        Ok(())
    }

    /// # Errors
    /// Any [`PartitionError`] from layout validation.
    pub fn to_bytes(&self, flash: FlashSize) -> Result<Vec<u8>, PartitionError> {
        self.validate(flash)?;
        let mut out = Vec::with_capacity(PARTITION_TABLE_LEN);
        for p in &self.entries {
            out.extend(ENTRY_MAGIC);
            out.extend([p.ptype, p.subtype]);
            out.extend(p.offset.to_le_bytes());
            out.extend(p.size.to_le_bytes());
            let mut name = [0u8; NAME_LEN];
            name[..p.name.len()].copy_from_slice(p.name.as_bytes());
            out.extend(name);
            out.extend(p.flags.to_le_bytes());
        }
        let digest = Md5::digest(&out);
        out.extend(MD5_MAGIC);
        out.extend([ERASED; 14]);
        out.extend(digest);
        out.resize(PARTITION_TABLE_LEN, ERASED);
        Ok(out)
    }

    /// # Errors
    /// [`PartitionError::BadEntryMagic`] or [`PartitionError::Md5Mismatch`] for a corrupted table.
    pub fn parse(bytes: &[u8]) -> Result<PartitionTable, PartitionError> {
        let mut entries = Vec::new();
        for (index, e) in bytes.chunks_exact(ENTRY_LEN).enumerate() {
            match [e[0], e[1]] {
                ENTRY_MAGIC => entries.push(parse_entry(e)),
                MD5_MAGIC => {
                    let covered = &bytes[..index * ENTRY_LEN];
                    if Md5::digest(covered).as_slice() != &e[16..32] { return Err(PartitionError::Md5Mismatch); }
                }
                [ERASED, ERASED] => break,
                _ => return Err(PartitionError::BadEntryMagic { index }),
            }
        }
        if entries.is_empty() { return Err(PartitionError::Empty); }
        Ok(PartitionTable { entries })
    }
}

fn parse_entry(e: &[u8]) -> Partition {
    let name_end = e[12..28].iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    Partition {
        name: String::from_utf8_lossy(&e[12..12 + name_end]).into_owned(),
        ptype: e[2], subtype: e[3],
        offset: u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
        size: u32::from_le_bytes([e[8], e[9], e[10], e[11]]),
        flags: u32::from_le_bytes([e[28], e[29], e[30], e[31]]),
    }
}

/// Stock Xteink X4 layout (hardware research §3.2; CrossPoint `partitions.csv`).
#[must_use]
pub fn xteink_x4_stock() -> PartitionTable {
    let p = |name: &str, ptype, subtype, offset, size| Partition { name: name.into(), ptype, subtype, offset, size, flags: 0 };
    PartitionTable { entries: vec![
        p("nvs", TYPE_DATA, SUBTYPE_NVS, 0x9000, 0x5000),
        p("otadata", TYPE_DATA, SUBTYPE_DATA_OTA, 0xE000, 0x2000),
        p("app0", TYPE_APP, SUBTYPE_OTA_0, 0x1_0000, 0x64_0000),
        p("app1", TYPE_APP, SUBTYPE_OTA_1, 0x65_0000, 0x64_0000),
        p("spiffs", TYPE_DATA, SUBTYPE_SPIFFS, 0xC9_0000, 0x36_0000),
        p("coredump", TYPE_DATA, SUBTYPE_COREDUMP, 0xFF_0000, 0x1_0000),
    ] }
}
```
`lib.rs` : `pub mod partition; pub use partition::{Partition, PartitionError, PartitionTable};`.

- [ ] **Step 5: Lancer les tests**

Run: `cargo test -p firmware`
Expected: PASS. Si `x4_stock_matches_esp_idf_byte_for_byte` échoue, afficher les deux en hexadécimal (`xxd`) et corriger **notre** générateur : la sortie d'ESP-IDF fait foi.

- [ ] **Step 6: Commit**

```bash
git add tools/docker/esp-idf tools/fixtures tests/fixtures/partitions tools/conformance/versions.env
git commit -m "feat(firmware): ESP-IDF-compatible partition tables with X4 stock layout" -- crates/firmware tools/docker/esp-idf tools/fixtures tests/fixtures/partitions tools/conformance/versions.env
```

---

### Task 12: `inkemu inspect` — afficher et valider une image firmware

Première commande utile à l'utilisateur final : elle dit ce que contient un `.bin` (puce cible, version, segments) ou **pourquoi** il sera refusé. Les noms de régions mémoire sont ceux de la carte mémoire de l'ESP32-C3 (manuel technique, chapitre « System and Memory ») ; pour les autres puces, la colonne affiche `-` en attendant leur SoC (SP7).

**Files:**
- Create: `crates/emu-cli/src/inspect.rs`, `crates/emu-cli/tests/inspect_cli.rs`
- Modify: `crates/emu-cli/src/lib.rs`, `crates/emu-cli/src/main.rs`, `crates/emu-cli/Cargo.toml`

**Interfaces:**
- Consumes: `firmware::{FirmwareImage, FirmwareError, ChipId}`, `firmware::builder` (tests, feature `test-support`).
- Produces: `emu_cli::inspect::render(image: &FirmwareImage, file_name: &str, file_len: usize) -> String` ; CLI `inkemu inspect <FILE>` (code 0 si l'image est valide, 2 sinon, message d'erreur sur stderr).

- [ ] **Step 1: Dépendances de test**

Dans `crates/emu-cli/Cargo.toml` :
```toml
[dev-dependencies]
assert_cmd.workspace = true
predicates.workspace = true
tempfile = "3"
firmware = { workspace = true, features = ["test-support"] }
```

- [ ] **Step 2: Tests (échouent)**

`crates/emu-cli/tests/inspect_cli.rs` :
```rust
use assert_cmd::Command;
use firmware::builder::{build_image, ImageSpec};
use predicates::str::contains;

fn write_temp(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fw.bin");
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

#[test]
fn inspect_describes_a_valid_image() {
    let (_d, path) = write_temp(&build_image(&ImageSpec::default()));
    Command::cargo_bin("inkemu").unwrap().arg("inspect").arg(&path).assert().success()
        .stdout(contains("ESP32-C3"))
        .stdout(contains("0x4038086e"))
        .stdout(contains("16 MB"))
        .stdout(contains("demo 1.0.0"))
        .stdout(contains("SHA-256 appended (verified)"))
        .stdout(contains("IRAM"));
}

#[test]
fn inspect_explains_encrypted_ota() {
    let mut bytes = b"XOTA".to_vec();
    bytes.resize(1024, 0);
    let (_d, path) = write_temp(&bytes);
    Command::cargo_bin("inkemu").unwrap().arg("inspect").arg(&path).assert().code(2)
        .stderr(contains("encrypted OTA update"));
}

#[test]
fn inspect_explains_wrong_format() {
    let (_d, path) = write_temp(&[0x7f, b'E', b'L', b'F'].repeat(16));
    Command::cargo_bin("inkemu").unwrap().arg("inspect").arg(&path).assert().code(2)
        .stderr(contains("expected 0xe9"));
}

#[test]
fn inspect_reports_missing_file() {
    Command::cargo_bin("inkemu").unwrap().args(["inspect", "/nonexistent/fw.bin"]).assert().code(2)
        .stderr(contains("cannot read /nonexistent/fw.bin"));
}

/// Real-firmware check, opt-in: set INKEMU_REAL_FIRMWARE to a CrossPoint X3/X4 .bin (never committed).
#[test]
#[ignore = "needs a third-party firmware file; run with INKEMU_REAL_FIRMWARE=<path> cargo test -- --ignored"]
fn inspect_real_crosspoint_firmware() {
    let path = std::env::var("INKEMU_REAL_FIRMWARE").expect("set INKEMU_REAL_FIRMWARE");
    Command::cargo_bin("inkemu").unwrap().arg("inspect").arg(path).assert().success()
        .stdout(contains("ESP32-C3"))
        .stdout(contains("DROM"));
}
```

- [ ] **Step 3: Constater l'échec** — Run: `cargo test -p emu-cli --test inspect_cli` → la sous-commande `inspect` n'existe pas.

- [ ] **Step 4: Implémenter `inspect.rs`**

```rust
//! Human-readable description of a firmware image.
use firmware::{ChipId, FirmwareImage};
use std::fmt::Write;

/// ESP32-C3 address ranges (Technical Reference Manual, "System and Memory").
const ESP32C3_REGIONS: &[(u32, u32, &str)] = &[
    (0x3C00_0000, 0x3C80_0000, "DROM (flash data)"),
    (0x3FC8_0000, 0x3FCE_0000, "DRAM"),
    (0x4037_C000, 0x403E_0000, "IRAM"),
    (0x4200_0000, 0x4280_0000, "IROM (flash code)"),
    (0x5000_0000, 0x5000_2000, "RTC fast memory"),
];

fn region(chip: ChipId, addr: u32) -> &'static str {
    if chip != ChipId::Esp32C3 { return "-"; }
    ESP32C3_REGIONS.iter().find(|(lo, hi, _)| (*lo..*hi).contains(&addr)).map_or("unknown", |r| r.2)
}

#[must_use]
pub fn render(img: &FirmwareImage, file_name: &str, file_len: usize) -> String {
    let mut s = String::new();
    // INVARIANT: writing to a String cannot fail.
    let _ = writeln!(s, "File:        {file_name} ({file_len} bytes)");
    let _ = writeln!(s, "Format:      ESP-IDF application image");
    let _ = writeln!(s, "Chip:        {}", img.chip);
    let _ = writeln!(s, "Entry point: {:#010x}", img.entry);
    let _ = writeln!(s, "Flash:       {} MB, {:?}, frequency code {:#x}", img.flash_size.megabytes(), img.spi_mode, img.flash_freq_raw);
    let _ = writeln!(s, "Hash:        {}", if img.hash_appended { "SHA-256 appended (verified)" } else { "none (checksum verified)" });
    match &img.app_desc {
        Some(d) => { let _ = writeln!(s, "App:         {} {} (ESP-IDF {}), built {} {}", d.project_name, d.version, d.idf_ver, d.date, d.time); }
        None => { let _ = writeln!(s, "App:         no application descriptor"); }
    }
    let _ = writeln!(s, "Segments:");
    let _ = writeln!(s, "  #  load address  size       region");
    for (i, seg) in img.segments.iter().enumerate() {
        let _ = writeln!(s, "  {i:<2} {:#010x}    {:<10} {}", seg.load_addr, seg.data.len(), region(img.chip, seg.load_addr));
    }
    s
}
```
`lib.rs` : ajouter `pub mod inspect;`.

- [ ] **Step 5: Sous-commande `inspect`**

Dans `main.rs`, ajouter la variante :
```rust
    /// Describe a firmware image and check that inkemu can load it.
    Inspect { file: PathBuf },
```
et le bras :
```rust
        Command::Inspect { file } => run_inspect(&file),
```
avec :
```rust
fn run_inspect(path: &std::path::Path) -> ExitCode {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => { eprintln!("error: cannot read {}: {e}", path.display()); return ExitCode::from(EXIT_ERROR); }
    };
    match firmware::FirmwareImage::parse(&bytes) {
        Ok(img) => {
            let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
            print!("{}", emu_cli::inspect::render(&img, &name, bytes.len()));
            ExitCode::SUCCESS
        }
        Err(e) => { eprintln!("error: {}: {e}", path.display()); ExitCode::from(EXIT_ERROR) }
    }
}
```
Ajouter `firmware.workspace = true` aux dépendances (normales) de `emu-cli`.

- [ ] **Step 6: Lancer les tests**

Run: `cargo test -p emu-cli`
Expected: PASS (le test réel reste ignoré).

- [ ] **Step 7: Vérification sur un vrai firmware (avec accord)**

**Demander l'accord de l'utilisateur avant de télécharger**, en précisant : fichier `crosspoint-1.6.5-x3-x4.bin` (environ 5,6 Mo), source `github.com/crosspoint-reader/crosspoint-reader`, release `v1.6.5`. Après accord :
```bash
mkdir -p "$SCRATCH/fw" && gh release download v1.6.5 -R crosspoint-reader/crosspoint-reader -p '*x3-x4.bin' -D "$SCRATCH/fw"
INKEMU_REAL_FIRMWARE="$(ls "$SCRATCH"/fw/*x3-x4.bin)" cargo test -p emu-cli --test inspect_cli -- --ignored
cargo run -q -p emu-cli -- inspect "$SCRATCH"/fw/*x3-x4.bin
```
`$SCRATCH` est un répertoire hors du dépôt : le binaire tiers n'est jamais committé. Expected : test PASS ; la sortie montre `ESP32-C3`, 16 Mo, DIO, et des segments DROM/DRAM/IROM/IRAM cohérents avec la recherche matérielle (§3.1).

- [ ] **Step 8: Commit**

```bash
git commit -m "feat(cli): inkemu inspect describes firmware images and explains rejections" -- crates/emu-cli
```

---

## Fin du plan SP1a — vérification globale

- [ ] `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [ ] `cargo llvm-cov --workspace --fail-under-lines 80` ; `decode.rs`, `decode_c.rs`, `image.rs`, `app_desc.rs` à 100 %.
- [ ] `cargo deny check`
- [ ] `tools/conformance/run-arch-test.sh` → code 0.
- [ ] CI GitHub verte sur `main`.

Le plan SP1b sera rédigé une fois l'ADR 0001 accepté : ses décisions (ROM embarquée ou téléchargée, origine des SVD) conditionnent sa première tâche. Il reprend tout ce que la spec §10.2 prévoit et que SP1a ne couvre pas :

| Élément de la spec | Raison du report |
|---|---|
| `svd-regs` (génération depuis les SVD) | dépend de l'ADR 0001 |
| CSR et contrôleur d'interruptions propres à l'ESP32-C3, priorités | à arbitrer par le lockstep QEMU |
| `soc-esp32c3` (ROM, cache/MMU XIP et coût de fetch, SPI1, horloges, eFuse, matrice d'interruptions, SYSTIMER, TIMG + watchdogs, RTC_CNTL, GPIO, UART0, USB-Serial-JTAG) | sous-projet SoC |
| Assemblage de la flash (bootloader embarqué + table + application) | nécessite le bootloader construit en Docker |
| `boards` : profils TOML `x4` et `x3` | relie SoC et puces |
| `inkemu run` (`--max-cycles`, `--strict-mmio`), `EventLog`, enregistrement et rejeu | nécessite le SoC |
| Harnais lockstep QEMU, CoreMark ±15 % (recalage de `DatasheetTiming`), politique d'alignement du C3 | nécessitent le SoC |
| Critères d'acceptation 2 à 6 de la spec §10.3 | idem |
