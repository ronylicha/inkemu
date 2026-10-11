# SP1b-1 — SoC ESP32-C3 minimal : la ROM démarre et affiche sa bannière — Plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `inkemu run --device x4` exécute la vraie ROM masquée de l'ESP32-C3 depuis le vecteur de reset et affiche sa bannière de démarrage sur la console, de façon déterministe.

**Architecture:** `svd-regs` fournit la table des registres générée depuis le SVD d'Espressif. `soc-esp32c3` assemble la carte mémoire réelle (ROM, SRAM vue par deux bus, mémoire RTC, fenêtre des périphériques), convertit l'ELF de ROM en image, et modélise les périphériques que la ROM touche avant sa bannière ; tout autre registre lit sa valeur de reset SVD et produit un avertissement nommé. `boards` décrit les appareils en TOML et instancie la machine. `emu-cli` gère le cache de ROM (téléchargement vérifié) et expose `inkemu run`.

**Tech Stack:** Rust 1.99.0 (édition 2024), `roxmltree` (générateur SVD), `goblin` (ELF), `sha2`, `ureq` + `flate2` + `tar` + `directories` (cache de ROM, côté hôte uniquement), `serde` + `toml` (profils), `clap`, `thiserror`, `assert_cmd`/`predicates`/`tempfile` (tests).

**Spec:** `docs/superpowers/specs/2026-10-11-inkemu-design.md` (§3.1 à §3.4, §4.3, §5.1, §5.2, §6.4, §7, §8, §10). Faits matériels : `docs/research/esp32c3-boot-chain.md`, `docs/research/esp32c3-rom-elfs.md`, `docs/research/esp32c3-svd.md`. Décision de licence : `docs/adr/0001-espressif-rom-and-svd-licensing.md`.

## Global Constraints

- Toolchain épinglée : Rust **1.99.0**, édition **2024**. Lints du workspace inchangés (`clippy::all` en erreur, `pedantic` en avertissement, CI avec `-D warnings`, `unsafe_code = "forbid"`).
- Licence **Apache-2.0** ; chaque dépendance passe `cargo deny check`. Toute licence hors liste blanche se traite par une exception **nommée par crate** dans `deny.toml`, jamais en élargissant la liste.
- Dernière version stable de chaque dépendance ajoutée : la vérifier avec `cargo info <crate>` avant de l'écrire dans `Cargo.toml`.
- **Aucun binaire de ROM dans le dépôt.** La ROM vient de l'archive épinglée `esp-rom-elfs-20260528.tar.gz` (SHA-256 `caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46`), vérifiée **avant** toute extraction. Une ROM dont l'empreinte diffère n'est jamais chargée.
- Le SVD est copié **à l'identique** depuis `espressif/svd` release `2024-05-09` (`esp32c3.svd`, SHA-256 `9ef33dbff56d807996fd8edfa4aa4ad008b5a861c241d36280caa0dedbbca41a`).
- QEMU (GPL) et son image de ROM ne sont ni liés, ni copiés, ni distribués. Aucun code ni commentaire de QEMU n'est recopié : seuls des faits (adresses, valeurs) en sont tirés.
- Règles de dépendance : `emu-core` ne dépend d'aucune crate du workspace ; `svd-regs` n'a aucune dépendance et reste `no_std` ; `soc-esp32c3` ne fait **aucune entrée-sortie hôte** (ni fichier, ni réseau, ni horloge) — elle doit compiler pour WASM au sous-projet 6.
- Déterminisme : aucune horloge murale, aucun aléa, aucune itération de `HashMap` dans le chemin émulé ; les collections ordonnées sont des `Vec` ou des `BTreeMap`.
- Registres non émulés (§5.2) : lecture = valeur de reset du SVD, écriture ignorée, avertissement **dédoublonné** qui nomme le périphérique, le registre, l'adresse et le PC ; `--strict-mmio` arrête la machine.
- Chaque comportement matériel modélisé porte en commentaire **sa source** (section du rapport de recherche, symbole de ROM, en-tête ESP-IDF). Un comportement supposé est marqué `ASSUMPTION:` avec ce qui le lèverait.
- Erreurs : un `enum` `thiserror` par module d'entrée ; chaque message dit **la cause, puis la correction**, séparées par un point-virgule.
- Aucun `unwrap()`/`expect()` hors tests, sauf invariant documenté par `// INVARIANT:`. Nombres magiques interdits : constantes nommées.
- Couverture ≥ 80 % par crate.
- Commits : `type(scope): description`, uniquement les chemins de la tâche (`/usr/bin/git commit -- <chemins>`), message terminé par la ligne `Claude-Session: https://claude.ai/code/session_01AwFxZY77NkrWpZTUtrDjAQ`.
- Vérification d'une commande : rediriger la sortie vers un fichier puis lire `$?` ; ne jamais lire un code de sortie à travers un tube.

## Review Focus

- **ROM absente, altérée ou téléchargement corrompu** (hors ligne sans cache, archive tronquée, empreinte différente, fichier du cache modifié) : refus avec le nom du fichier, l'empreinte attendue, l'empreinte observée et la marche à suivre (`--rom-dir`) ; aucun fichier partiel laissé dans le cache → tests `archive_hash_mismatch_is_refused_and_nothing_is_written`, `tampered_cached_file_is_refused_with_both_hashes`, `download_failure_explains_the_offline_route` (Task 9).
- **Registre non émulé lu en boucle** (attente active sur un périphérique absent, des millions d'accès) : un seul avertissement par adresse et par sens, un compteur, une mémoire bornée → tests `repeated_access_warns_once_and_counts` et `distinct_diagnostics_are_capped` (Task 5).
- **Fichier fourni comme ROM qui n'en est pas une** (ELF Xtensa du S3, ELF quelconque, fichier tronqué, texte) : erreur nommée, jamais de panic ni d'allocation démesurée → tests `rejects_non_riscv_elf`, `rejects_wrong_chip_id`, `truncated_elf_is_an_error_not_a_panic` (Task 4).
- **Accès MMIO inhabituel** (octet ou demi-mot, adresse désalignée, trou de la fenêtre des périphériques, écriture en ROM) : comportement défini et signalé, jamais de panic → tests `sub_word_read_extracts_the_addressed_bytes`, `sub_word_write_is_reported`, `gap_in_peripheral_window_reads_zero_and_warns`, `store_to_rom_faults` (Task 5).
- **Exécution qui ne se termine pas ou sortie fermée** (ROM en attente infinie, `inkemu run | head`) : `--max-cycles` arrête proprement avec un message et le code 0, un CPU en attente sans réveil possible s'arrête avec le code 4 ; un `stdout` fermé termine sans panic → tests `max_cycles_ends_the_run_cleanly`, `idle_cpu_ends_the_run_and_says_why` et `closed_stdout_does_not_panic` (Task 11).

---

## Décisions de conception prises pour ce plan

| # | Décision | Raison | Coût si elle est fausse |
|---|---|---|---|
| D1 | SP1b est découpé en quatre plans : **1b-1** ROM → bannière (ce plan), **1b-2** flash + cache/MMU + SHA → le bootloader s'exécute, **1b-3** interruptions, timers, watchdogs, reset → un firmware affiche ses logs, **1b-4** lockstep QEMU, CoreMark, enregistrement/rejeu. | Chaque plan livre un jalon observable et se fusionne seul. | Faible : redécouper un plan non commencé. |
| D2 | `svd-regs` contient un fichier Rust **généré et committé** (`svd-gen`, contrôle `--check` en CI), sans champs de bits, sans type d'accès. | Rien dans §5.2 n'a besoin des champs ; un `build.rs` qui lit du XML ralentit chaque build. Les champs s'ajoutent plus tard dans une seconde table sans casser l'API. | Moyen : ajouter les champs au générateur (≈ +3 500 lignes générées). |
| D3 | La table distingue une valeur de reset **documentée** d'un zéro par défaut (`reset_explicit`) : 83 % des registres du SVD n'ont pas de valeur de reset. | L'avertissement doit dire quand le zéro renvoyé n'est pas une valeur documentée. | Nul. |
| D4 | Révision de puce par défaut des X4/X3 : **v0.3**, donc ROM `esp32c3_rev3`. La révision réelle des appareils est inconnue. | C'est la révision la plus répandue et la seule dont l'image reconstruite est vérifiée octet pour octet. | Faible : le profil se change en une ligne ; les trois ROM sont sélectionnables. |
| D5 | L'image de ROM est **reconvertie depuis l'ELF à chaque lancement** ; le cache ne contient que l'ELF vérifié. | Un seul fichier et une seule empreinte par ROM dans le cache ; la conversion prend quelques millisecondes et son résultat est lui aussi vérifié. | Nul. |
| D6 | Les CSR propres au C3 vivent dans `cpu-rv32` derrière `HartConfig::vendor_csrs` (la spec §3.2 les place dans cette crate). Ce plan n'ajoute que ceux que la ROM utilise : `ustatus` et le compteur de performance (`0x7E0`–`0x7E2`, miroirs `0x800`–`0x802`). PMP et déclencheurs arrivent avec l'application (1b-3). | YAGNI ; le compteur est indispensable, sinon `ets_delay_us` boucle. | Faible. |
| D7 | La livraison des interruptions, le temps propre aux périphériques (débit de l'UART, timers), le reset logiciel et les watchdogs sont reportés à 1b-3. Ici la matrice d'interruptions mémorise sa configuration et l'UART émet instantanément. | Rien ne lève d'interruption avant la bannière. **Conséquence assumée :** le temps de démarrage émulé est sous-estimé tant que l'UART ne prend pas son temps réel. | Nul pour ce jalon ; à traiter en 1b-3. |
| D8 | `--max-cycles` compte les **cycles d'horloge du CPU** (`Hart::cycles`). | C'est l'unité du temps virtuel de la spec (§3.1) ; la conversion vers les autres horloges est l'affaire de 1b-3. | Faible. |
| D9 | Une écriture de 8 ou 16 bits dans un registre de périphérique écrit le registre entier, les autres octets à zéro, avec un avertissement. | Le matériel exige des accès de 32 bits ; le comportement réel d'un accès partiel n'est pas documenté. Le signaler vaut mieux que de le deviner en silence. | Faible : un firmware qui en dépend se verra à l'avertissement. |
| D10 | Une écriture en ROM lève une faute d'accès en écriture. | Comportement réel non établi ; la faute rend le bug visible. | Faible. |
| D11 | Console affichée par défaut : **UART0**. La ROM écrit aussi sur USB-Serial-JTAG ; `--console usb-serial-jtag` choisit ce canal. | Le journal de référence de QEMU est celui de l'UART0. Le canal réellement utilisé par les firmwares Xteink sera établi en 1b-3. | Faible. |
| D12 | Un seul SVD est copié dans le dépôt (`esp32c3.svd`) ; celui du S3 le sera au sous-projet 7. L'ADR est amendé. | YAGNI : 2,8 Mo inutilisés. | Nul. |

## Points reportés de SP1a : où ils sont traités

Source : `docs/superpowers/notes/2026-10-11-sp1a-carryover.md`.

| Point | Plan |
|---|---|
| `Bus::page_version` tenu honnêtement, alias IRAM/DRAM | **1b-1**, Task 5 |
| Écriture de `mcycle` prioritaire sur l'incrément | **1b-1**, Task 3 |
| `debug_assert` dans `set_interrupt_pending` | **1b-1**, Task 3 |
| `inspect` : `Display` de `SpiMode`, EPIPE | **1b-1**, Task 11 (EPIPE, pour `run` et `inspect`) ; `SpiMode` en 1b-2 |
| `fetch_stall`, cache flash ; heuristique de dump flash ; `BadFlashSize` | 1b-2 |
| `DecodeCache::clear()` et `Debug` manuel ; latence d'interruption ; PMP et déclencheurs ; test CLI dépendant de `pmpaddr` ; politique d'alignement | 1b-3 |
| Erreur `Misaligned` du bus traduite en faute d'accès ; `rvtest` (chemins, `tohost`, `p_paddr`, empreinte) | 1b-4 |
| Hygiène CI (épinglages, `--locked`, couverture par fichier) | 1b-4 |

---

## File Structure

```
third_party/espressif-svd/
  esp32c3.svd                      copie à l'identique (ADR 0001)
  LICENSE  NOTICE  PROVENANCE.md
crates/svd-gen/                    générateur (publish = false)
  src/main.rs                      CLI : génère ou vérifie (--check)
  src/expand.rs                    SVD → liste de registres (dim, derivedFrom, alias)
  src/emit.rs                      liste → source Rust
crates/svd-regs/                   #![no_std], aucune dépendance
  src/lib.rs                       RegisterInfo, recherche
  src/esp32c3.rs                   @generated
crates/emu-core/src/peripheral.rs  trait Peripheral (MMIO par registres de 32 bits)
crates/cpu-rv32/src/
  csr.rs                           + VendorCsrs, mcycle prioritaire
  csr_esp32c3.rs                   ustatus, compteur de performance
crates/soc-esp32c3/src/
  lib.rs                           ré-exports
  memmap.rs                        constantes de la carte mémoire
  chip.rs                          ChipRevision
  rom.rs                           RomImage : ELF → image de 384 Kio
  diag.rs                          Diagnostic, Diagnostics (dédoublonnage, mode strict)
  bus.rs                           SocBus : régions, alias, répartition MMIO
  periph/mod.rs                    Peripherals (un champ par périphérique), Storage
  periph/intmatrix.rs  rtc_cntl.rs  system.rs  efuse.rs  gpio.rs  io_mux.rs  assist_debug.rs
  periph/uart.rs  usb_serial_jtag.rs
  machine.rs                       Esp32C3 : boucle d'exécution, consoles, empreinte de trace
crates/soc-esp32c3/tests/real_rom.rs   tests sur la vraie ROM (ignorés sans INKEMU_ROM_ELF_DIR)
crates/boards/
  profiles/x4.toml  x3.toml
  src/lib.rs                       DeviceProfile, chargement, validation, instanciation
crates/emu-cli/src/
  rom_store.rs                     manifeste, cache, téléchargement vérifié
  run.rs                           commande `run`
  main.rs                          + sous-commandes `run` et `rom`
tools/research/elf2rom.py          contre-épreuve Python de la conversion de ROM (déjà présent)
```

---

### Task 1: SVD copié dans le dépôt et générateur `svd-gen`

**Files:**
- Create: `third_party/espressif-svd/esp32c3.svd`, `third_party/espressif-svd/LICENSE`, `third_party/espressif-svd/NOTICE`, `third_party/espressif-svd/PROVENANCE.md`
- Create: `crates/svd-gen/Cargo.toml`, `crates/svd-gen/src/main.rs`, `crates/svd-gen/src/expand.rs`, `crates/svd-gen/src/emit.rs`
- Modify: `Cargo.toml` (membre `crates/svd-gen` ; dépendance `roxmltree`)

**Interfaces:**
- Consumes: rien.
- Produces (crate `svd-gen`, usage interne et Task 2) :
  ```rust
  pub struct Register { pub addr: u32, pub peripheral: String, pub name: String, pub reset_value: u32, pub reset_explicit: bool }
  pub struct Alias { pub addr: u32, pub peripheral: &'static str, pub register: &'static str }
  pub enum GenError { /* un variant par cause, voir Step 3 */ }
  pub fn expand(svd_xml: &str, aliases: &[Alias]) -> Result<Vec<Register>, GenError>;   // trié par adresse, adresses uniques
  pub fn emit(source_name: &str, source_sha256: &str, regs: &[Register]) -> String;
  ```
  CLI : `cargo run -p svd-gen` écrit `crates/svd-regs/src/esp32c3.rs` ; `cargo run -p svd-gen -- --check` sort en 1 si le fichier diffère.

Le prototype complet de ce générateur (parseur `roxmltree`, marcheur strict) est dans `docs/research/esp32c3-svd.md`, §4.4, fichier `svd-gen/src/via_roxmltree.rs`. Le porter avec ces écarts : pas de backend `svd-parser`, pas de champ `access`, pas de `size_bits` (le générateur échoue si un registre n'a pas `size` = `0x20`), un champ `reset_explicit`, et le nom du périphérique porté par chaque registre.

- [ ] **Step 1: Copier le SVD et vérifier son empreinte**

```bash
mkdir -p third_party/espressif-svd
curl -fsSL -o third_party/espressif-svd/esp32c3.svd \
  https://github.com/espressif/svd/releases/download/2024-05-09/esp32c3.svd
curl -fsSL -o third_party/espressif-svd/LICENSE \
  https://raw.githubusercontent.com/espressif/svd/be20aa12560889d6125d144cdb48cf615ac17628/LICENSE
sha256sum third_party/espressif-svd/esp32c3.svd
```
Expected: `9ef33dbff56d807996fd8edfa4aa4ad008b5a861c241d36280caa0dedbbca41a` et une taille de 1 366 305 octets. Si l'empreinte diffère, s'arrêter et le signaler : ne pas modifier la constante.

Écrire `third_party/espressif-svd/PROVENANCE.md` :

```markdown
# Provenance

| File | Source | Release | Commit | SHA-256 |
|---|---|---|---|---|
| `esp32c3.svd` | https://github.com/espressif/svd/releases/download/2024-05-09/esp32c3.svd | `2024-05-09` | `be20aa12560889d6125d144cdb48cf615ac17628` | `9ef33dbff56d807996fd8edfa4aa4ad008b5a861c241d36280caa0dedbbca41a` |

The file is an unmodified copy. `LICENSE` is the Apache-2.0 text of `espressif/svd` at the same commit.
`crates/svd-regs/src/esp32c3.rs` is generated from it by `crates/svd-gen`.
```

Écrire `third_party/espressif-svd/NOTICE` :

```text
ESP32-C3 CMSIS-SVD register description
Copyright Espressif Systems (Shanghai) Co., Ltd.
Licensed under the Apache License, Version 2.0 (see LICENSE in this directory).
Source: https://github.com/espressif/svd (release 2024-05-09).
```

- [ ] **Step 2: Écrire les tests du marcheur (`crates/svd-gen/src/expand.rs`, module `tests`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn device(peripherals: &str) -> String {
        format!(
            "<device><name>T</name><resetValue>0x00000000</resetValue><peripherals>{peripherals}</peripherals></device>"
        )
    }

    fn reg(name: &str, offset: &str, extra: &str) -> String {
        format!("<register><name>{name}</name><addressOffset>{offset}</addressOffset><size>0x20</size>{extra}</register>")
    }

    fn periph(name: &str, base: &str, registers: &str) -> String {
        format!("<peripheral><name>{name}</name><baseAddress>{base}</baseAddress><registers>{registers}</registers></peripheral>")
    }

    fn names(regs: &[Register]) -> Vec<(u32, String)> {
        regs.iter().map(|r| (r.addr, format!("{}.{}", r.peripheral, r.name))).collect()
    }

    #[test]
    fn plain_register_gets_absolute_address_and_explicit_reset() {
        let xml = device(&periph("UART0", "0x60000000", &reg("STATUS", "0x1C", "<resetValue>0xE000C000</resetValue>")));
        let regs = expand(&xml, &[]).unwrap();
        assert_eq!(regs.len(), 1);
        assert_eq!((regs[0].addr, regs[0].reset_value, regs[0].reset_explicit), (0x6000_001C, 0xE000_C000, true));
    }

    #[test]
    fn missing_reset_value_inherits_the_device_default_and_is_flagged() {
        let xml = device(&periph("GPIO", "0x60004000", &reg("STRAP", "0x38", "")));
        let regs = expand(&xml, &[]).unwrap();
        assert_eq!((regs[0].reset_value, regs[0].reset_explicit), (0, false));
    }

    #[test]
    fn dim_array_expands_percent_s_with_a_range_index() {
        let extra = "<dim>3</dim><dimIncrement>0x10</dimIncrement><dimIndex>0-2</dimIndex>";
        let xml = device(&periph("DMA", "0x6003F000", &reg("INT_RAW_CH%s", "0x0", extra)));
        assert_eq!(
            names(&expand(&xml, &[]).unwrap()),
            [(0x6003_F000, "DMA.INT_RAW_CH0".into()), (0x6003_F010, "DMA.INT_RAW_CH1".into()), (0x6003_F020, "DMA.INT_RAW_CH2".into())]
        );
    }

    #[test]
    fn dim_array_without_index_drops_the_brackets() {
        let extra = "<dim>2</dim><dimIncrement>0x4</dimIncrement>";
        let xml = device(&periph("AES", "0x6003A000", &reg("IV_MEM[%s]", "0x50", extra)));
        assert_eq!(
            names(&expand(&xml, &[]).unwrap()),
            [(0x6003_A050, "AES.IV_MEM0".into()), (0x6003_A054, "AES.IV_MEM1".into())]
        );
    }

    #[test]
    fn derived_peripheral_copies_the_parent_registers_at_its_own_base() {
        let parent = periph("UART0", "0x60000000", &reg("FIFO", "0x0", ""));
        let child = "<peripheral derivedFrom=\"UART0\"><name>UART1</name><baseAddress>0x60010000</baseAddress></peripheral>";
        let regs = expand(&device(&format!("{parent}{child}")), &[]).unwrap();
        assert_eq!(names(&regs), [(0x6000_0000, "UART0.FIFO".into()), (0x6001_0000, "UART1.FIFO".into())]);
    }

    #[test]
    fn output_is_sorted_by_address_whatever_the_document_order() {
        let regs = format!("{}{}", reg("B", "0x8", ""), reg("A", "0x4", ""));
        let out = expand(&device(&periph("P", "0x60000000", &regs)), &[]).unwrap();
        assert_eq!(out.iter().map(|r| r.addr).collect::<Vec<_>>(), [0x6000_0004, 0x6000_0008]);
    }

    #[test]
    fn listed_alias_keeps_the_winner_and_drops_the_other_register() {
        let a = periph("APB_CTRL", "0x60026000", &reg("RND_DATA", "0xB0", ""));
        let b = periph("RNG", "0x60026000", &reg("DATA", "0xB0", ""));
        let alias = [Alias { addr: 0x6002_60B0, peripheral: "RNG", register: "DATA" }];
        let regs = expand(&device(&format!("{a}{b}")), &alias).unwrap();
        assert_eq!(names(&regs), [(0x6002_60B0, "RNG.DATA".into())]);
    }

    #[test]
    fn unlisted_address_collision_is_an_error_naming_both_registers() {
        let a = periph("A", "0x60000000", &reg("X", "0x0", ""));
        let b = periph("B", "0x60000000", &reg("Y", "0x0", ""));
        let err = expand(&device(&format!("{a}{b}")), &[]).unwrap_err().to_string();
        assert!(err.contains("A.X") && err.contains("B.Y") && err.contains("0x60000000"), "{err}");
    }

    #[test]
    fn unsupported_constructs_are_rejected_instead_of_being_skipped() {
        let cluster = "<peripheral><name>P</name><baseAddress>0x0</baseAddress><registers><cluster><name>C</name></cluster></registers></peripheral>";
        assert!(matches!(expand(&device(cluster), &[]), Err(GenError::Unsupported { .. })));
        let wide = periph("P", "0x0", "<register><name>R</name><addressOffset>0x0</addressOffset><size>0x10</size></register>");
        assert!(matches!(expand(&device(&wide), &[]), Err(GenError::Unsupported { .. })));
        let orphan = "<peripheral derivedFrom=\"NOPE\"><name>Q</name><baseAddress>0x0</baseAddress></peripheral>";
        assert!(matches!(expand(&device(orphan), &[]), Err(GenError::UnknownParent { .. })));
    }

    #[test]
    fn numbers_accept_hex_and_decimal_and_reject_garbage() {
        assert_eq!(parse_number("0x1C").unwrap(), 0x1C);
        assert_eq!(parse_number("28").unwrap(), 28);
        assert!(parse_number("twelve").is_err());
    }
}
```

- [ ] **Step 3: Lancer les tests, constater l'échec de compilation**

Run: `cargo test -p svd-gen > /tmp/svd-gen.log 2>&1; echo "exit=$?"`
Expected: `exit=101`, erreurs « cannot find function `expand` ».

- [ ] **Step 4: Implémenter `expand.rs`, `emit.rs`, `main.rs`**

`crates/svd-gen/Cargo.toml` :

```toml
[package]
name = "svd-gen"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[lints]
workspace = true

[dependencies]
roxmltree.workspace = true
sha2.workspace = true
thiserror.workspace = true
```

`GenError` (dans `expand.rs`) — un message, une cause, une correction :

```rust
#[derive(Debug, thiserror::Error)]
pub enum GenError {
    #[error("the SVD is not well-formed XML ({detail}); re-download it from the URL in PROVENANCE.md")]
    Xml { detail: String },
    #[error("<{element}> in {context} has no <{child}>; the SVD layout changed, review the generator before bumping the file")]
    Missing { element: &'static str, context: String, child: &'static str },
    #[error("cannot read the number `{text}`; the generator accepts decimal and 0x-prefixed hexadecimal only")]
    Number { text: String },
    #[error("unsupported SVD construct in {context}: {what}; extend the generator instead of skipping it")]
    Unsupported { context: String, what: String },
    #[error("peripheral {child} derives from {parent}, which is not defined before it; check the derivedFrom attribute")]
    UnknownParent { child: String, parent: String },
    #[error("{first} and {second} are both at {addr:#010x}; add an alias rule naming the register to keep")]
    Collision { first: String, second: String, addr: u32 },
}
```

Règles du marcheur (toutes exercées par les tests ci-dessus) :

1. Valeur de reset par défaut = `<resetValue>` du `<device>`.
2. Pour chaque `<peripheral>` dans l'ordre du document : s'il a `derivedFrom`, il reprend les registres du parent déjà vu (sinon `UnknownParent`) ; un parent qui dérive lui-même d'un autre → `Unsupported`.
3. Tout enfant de `<registers>` autre que `<register>` → `Unsupported`. Un `<size>` différent de `0x20` → `Unsupported`.
4. `dim` : indices `0..dim` si `<dimIndex>` est absent, sinon une plage `a-b` ; toute autre forme (liste à virgules) → `Unsupported`. `%s` est remplacé par l'indice et `[%s]` aussi, crochets retirés. L'adresse avance de `dimIncrement` par élément.
5. Après expansion : tri par adresse ; deux registres à la même adresse → on garde celui que nomme la règle d'alias de cette adresse, sinon `Collision`.

`emit` produit exactement ce format (une ligne par registre, pour des diffs lisibles) :

```rust
// @generated by `cargo run -p svd-gen` from third_party/espressif-svd/esp32c3.svd
// (sha256 9ef33dbff56d807996fd8edfa4aa4ad008b5a861c241d36280caa0dedbbca41a). Do not edit.
// Register data: Copyright Espressif Systems (Shanghai) Co., Ltd., Apache-2.0 (espressif/svd).
use crate::RegisterInfo;

pub(crate) static REGISTERS: [RegisterInfo; 2605] = [
    RegisterInfo { addr: 0x6000_0000, peripheral: "UART0", name: "FIFO", reset_value: 0x0000_0000, reset_explicit: false },
    // …
];
```

`main.rs` : chemins dérivés de `env!("CARGO_MANIFEST_DIR")` (`../../third_party/espressif-svd/esp32c3.svd` en entrée, `../svd-regs/src/esp32c3.rs` en sortie). Il calcule le SHA-256 du SVD et **refuse de générer** s'il diffère de la constante `ESP32C3_SVD_SHA256`. La seule règle d'alias est `Alias { addr: 0x6002_60B0, peripheral: "RNG", register: "DATA" }` (ESP-IDF définit `RNG_DATA_REG` comme `SYSCON_RND_DATA_REG` : c'est le même registre). Sans argument il écrit le fichier ; avec `--check` il compare et sort en 1 avec le message `crates/svd-regs/src/esp32c3.rs is stale; run cargo run -p svd-gen`.

Ajouter dans le `Cargo.toml` racine : le membre `crates/svd-gen` et `roxmltree` à sa dernière version stable (`cargo info roxmltree`) dans `[workspace.dependencies]`. Le fichier de sortie n'est écrit qu'à la Task 2 : dans cette tâche, seuls `expand` et `emit` sont exercés, par les tests.

- [ ] **Step 5: Lancer les tests du générateur**

Run: `cargo test -p svd-gen > /tmp/svd-gen.log 2>&1; echo "exit=$?"`
Expected: `exit=0`, 10 tests passent.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add third_party/espressif-svd crates/svd-gen Cargo.toml Cargo.lock
/usr/bin/git commit -m "feat(svd-gen): vendor the ESP32-C3 SVD and add the register table generator" -- third_party/espressif-svd crates/svd-gen Cargo.toml Cargo.lock
```

---

### Task 2: `svd-regs`, la table des registres

**Files:**
- Create: `crates/svd-regs/Cargo.toml`, `crates/svd-regs/src/lib.rs`, `crates/svd-regs/src/esp32c3.rs` (généré), `crates/svd-regs/src/tests.rs`
- Modify: `Cargo.toml` (membre `crates/svd-regs` et `svd-regs` dans `[workspace.dependencies]`), `.github/workflows/ci.yml` (étape `--check`)

**Interfaces:**
- Consumes: `cargo run -p svd-gen` (Task 1).
- Produces:
  ```rust
  #![no_std]
  #[derive(Debug, PartialEq, Eq)]
  pub struct RegisterInfo {
      pub addr: u32,                 // absolu, multiple de 4
      pub peripheral: &'static str,  // nom SVD : "UART0", "USB_DEVICE", "INTERRUPT_CORE0"…
      pub name: &'static str,        // nom développé : "STATUS", "INT_RAW_CH0"
      pub reset_value: u32,          // 0 si le SVD n'en donne pas
      pub reset_explicit: bool,      // true si le SVD donne la valeur
  }
  pub mod esp32c3 {
      pub fn lookup(addr: u32) -> Option<&'static RegisterInfo>;   // tout octet du registre
      pub fn registers() -> &'static [RegisterInfo];               // trié par adresse, unique
  }
  ```

- [ ] **Step 1: Écrire `lib.rs` sans la recherche, et les tests**

`crates/svd-regs/src/lib.rs` :

```rust
//! Register tables generated from Espressif's CMSIS-SVD files. No field data: see `docs/research/esp32c3-svd.md`, §5.3.
#![no_std]

/// One 32-bit memory-mapped register.
#[derive(Debug, PartialEq, Eq)]
pub struct RegisterInfo {
    pub addr: u32,
    pub peripheral: &'static str,
    pub name: &'static str,
    /// `0` when the SVD documents no reset value: check `reset_explicit` before trusting it.
    pub reset_value: u32,
    pub reset_explicit: bool,
}

/// Every register of these SVD files is 32 bits wide (the generator rejects any other size).
const REGISTER_BYTES: u32 = 4;

fn lookup_in(table: &'static [RegisterInfo], addr: u32) -> Option<&'static RegisterInfo> {
    let index = table.partition_point(|r| r.addr <= addr).checked_sub(1)?;
    let reg = table.get(index)?;
    (addr - reg.addr < REGISTER_BYTES).then_some(reg)
}

/// ESP32-C3, from `esp32c3.svd` (espressif/svd release 2024-05-09).
pub mod esp32c3 {
    #[rustfmt::skip]
    #[path = "../esp32c3.rs"]
    mod generated;

    use crate::RegisterInfo;

    /// The register containing `addr`, if the SVD documents one there.
    #[must_use]
    pub fn lookup(addr: u32) -> Option<&'static RegisterInfo> {
        crate::lookup_in(&generated::REGISTERS, addr)
    }

    /// All documented registers, sorted by address.
    #[must_use]
    pub fn registers() -> &'static [RegisterInfo] {
        &generated::REGISTERS
    }
}

#[cfg(test)]
mod tests;
```

Si `#[path]` sur un module imbriqué pose problème à `rustfmt`, placer plutôt le fichier généré dans `src/esp32c3/generated.rs` et le module `esp32c3` dans `src/esp32c3/mod.rs` ; adapter alors le chemin de sortie de `svd-gen`.

`crates/svd-regs/src/tests.rs` :

```rust
use crate::esp32c3::{lookup, registers};

/// Update these two numbers only after reading the generator diff of an SVD bump.
const REGISTER_COUNT: usize = 2605;
const UART_REGISTER_COUNT: usize = 33;

#[test]
fn table_is_sorted_unique_and_word_aligned() {
    assert_eq!(registers().len(), REGISTER_COUNT);
    for pair in registers().windows(2) {
        assert!(pair[0].addr + 4 <= pair[1].addr, "{:#x} overlaps {:#x}", pair[0].addr, pair[1].addr);
    }
    assert!(registers().iter().all(|r| r.addr % 4 == 0));
}

#[test]
fn every_byte_of_a_register_resolves_to_it_and_gaps_resolve_to_nothing() {
    for (i, r) in registers().iter().enumerate() {
        for byte in 0..4 {
            assert_eq!(lookup(r.addr + byte), Some(r));
        }
        let next = registers().get(i + 1).map_or(u32::MAX, |n| n.addr);
        if r.addr + 4 < next {
            assert_eq!(lookup(r.addr + 4), None, "gap after {:#x}", r.addr);
        }
    }
    assert_eq!(lookup(0), None);
    assert_eq!(lookup(0x5fff_ffff), None);
}

fn expect(addr: u32, peripheral: &str, name: &str, reset: u32, explicit: bool) {
    let r = lookup(addr).unwrap_or_else(|| panic!("no register at {addr:#x}"));
    assert_eq!((r.peripheral, r.name, r.reset_value, r.reset_explicit), (peripheral, name, reset, explicit), "{addr:#x}");
}

/// Values cross-checked against the ESP-IDF v5.5 register headers (`docs/research/esp32c3-svd.md`, §4.3).
#[test]
fn known_registers_have_their_documented_reset_values() {
    expect(0x6000_001C, "UART0", "STATUS", 0xE000_C000, true);
    expect(0x6001_001C, "UART1", "STATUS", 0xE000_C000, true);
    expect(0x6002_3000, "SYSTIMER", "CONF", 0x4600_0000, true);
    expect(0x6000_8038, "RTC_CNTL", "RESET_STATE", 0x0000_3000, true);
    expect(0x600C_0008, "SYSTEM", "CPU_PER_CONF", 0x0000_000C, true);
}

#[test]
fn hardware_dependent_inputs_are_flagged_as_undocumented_zero() {
    expect(0x6000_4038, "GPIO", "STRAP", 0, false);
}

#[test]
fn the_single_address_collision_resolves_to_the_rng_register() {
    expect(0x6002_60B0, "RNG", "DATA", 0, false);
}

#[test]
fn derived_peripherals_have_the_same_layout_as_their_parent() {
    let offsets = |p: &str, base: u32| -> std::vec::Vec<(u32, &str)> {
        registers().iter().filter(|r| r.peripheral == p).map(|r| (r.addr - base, r.name)).collect()
    };
    let uart0 = offsets("UART0", 0x6000_0000);
    assert_eq!(uart0.len(), UART_REGISTER_COUNT);
    assert_eq!(uart0, offsets("UART1", 0x6001_0000));
    assert_eq!(offsets("TIMG0", 0x6001_F000), offsets("TIMG1", 0x6002_0000));
}

#[test]
fn blocks_absent_from_the_svd_are_absent_from_the_table() {
    // The flash MMU table has no SVD description: `soc-esp32c3` models it by hand.
    assert_eq!(lookup(0x600C_5000), None);
}
```

Le module de tests a besoin de `std` : ajouter `extern crate std;` en tête de `tests.rs`.

- [ ] **Step 2: Lancer les tests, constater l'échec**

Run: `cargo test -p svd-regs > /tmp/svd-regs.log 2>&1; echo "exit=$?"`
Expected: `exit=101` (le fichier généré n'existe pas).

- [ ] **Step 3: Générer la table**

Run: `cargo run -p svd-gen > /tmp/svd-gen-run.log 2>&1; echo "exit=$?"; wc -l crates/svd-regs/src/esp32c3.rs`
Expected: `exit=0`, environ 2 612 lignes.

- [ ] **Step 4: Lancer les tests et le contrôle de fraîcheur**

Run: `cargo test -p svd-regs > /tmp/svd-regs.log 2>&1; echo "exit=$?"; cargo run -p svd-gen -- --check > /tmp/svd-check.log 2>&1; echo "check=$?"`
Expected: `exit=0` (7 tests), `check=0`. Si une valeur attendue diffère de la table, ne pas corriger le test : comparer avec l'en-tête ESP-IDF cité dans le rapport et signaler l'écart.

Vérifier aussi que la crate compile sans `std` : `cargo build -p svd-regs --target wasm32-unknown-unknown` si la cible est installée (sinon `rustup target add wasm32-unknown-unknown` d'abord).

- [ ] **Step 5: Brancher le contrôle en CI**

Dans `.github/workflows/ci.yml`, job `check`, après `cargo test --workspace` :

```yaml
      - run: cargo run -p svd-gen -- --check
```

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/svd-regs Cargo.toml Cargo.lock .github/workflows/ci.yml
/usr/bin/git commit -m "feat(svd-regs): add the generated ESP32-C3 register table and its lookup" -- crates/svd-regs Cargo.toml Cargo.lock .github/workflows/ci.yml
```

---

### Task 3: `cpu-rv32` — CSR du C3, `mcycle` prioritaire, assembleur de test exposé

**Files:**
- Create: `crates/cpu-rv32/src/csr_esp32c3.rs`
- Modify: `crates/cpu-rv32/src/csr.rs`, `crates/cpu-rv32/src/hart.rs`, `crates/cpu-rv32/src/lib.rs`, `crates/cpu-rv32/Cargo.toml`, `crates/emu-cli/src/main.rs` (littéral `HartConfig`)

**Interfaces:**
- Consumes: `CsrFile`, `Hart`, `HartConfig` existants.
- Produces:
  ```rust
  // cpu_rv32
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
  pub enum VendorCsrs { #[default] None, Esp32C3 }
  pub struct HartConfig { pub misaligned: MisalignedPolicy, pub vendor_csrs: VendorCsrs }
  impl CsrFile { pub fn new(vendor: VendorCsrs) -> CsrFile; pub fn on_instruction_retired(&mut self, cycles_spent: u32); }
  // feature "test-support" : `pub mod asm` (encodeurs d'instructions pour les tests des autres crates)
  ```

Faits (source : `docs/research/esp32c3-boot-chain.md`, §B.1 et §B.2) : la ROM écrit `ustatus` (`0x000`) ; elle écrit 1 dans `0x800` et `0x801` puis lit `0x802` dans `ets_delay_us` (`ROM3:ets_delay_us@0x400462cc`), qui boucle tant que ce compteur n'a pas assez avancé. ESP-IDF utilise les mêmes registres par `0x7E0`–`0x7E2` (`rv_utils.h` : `CSR_PCER_MACHINE`, `CSR_PCMR_MACHINE`, `CSR_PCCR_MACHINE`).

- [ ] **Step 1: Écrire les tests du compteur (`csr_esp32c3.rs`, module `tests`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Cycles;

    #[test]
    fn counter_stays_frozen_until_it_is_enabled() {
        let c = Esp32C3Csrs::default();
        assert_eq!(c.read(PCCR_USER, Cycles(500), 40), Some(0));
    }

    #[test]
    fn counter_counts_cpu_cycles_once_the_cycle_event_and_the_enable_bit_are_set() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(PCER_USER, 1, Cycles(100), 10));
        assert!(c.write(PCMR_USER, 1, Cycles(110), 11));
        assert_eq!(c.read(PCCR_USER, Cycles(110), 11), Some(0));
        assert_eq!(c.read(PCCR_USER, Cycles(150), 20), Some(40));
    }

    #[test]
    fn machine_and_user_addresses_are_the_same_registers() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(PCER_MACHINE, 1, Cycles(0), 0));
        assert!(c.write(PCMR_MACHINE, 1, Cycles(0), 0));
        assert_eq!(c.read(PCER_USER, Cycles(0), 0), Some(1));
        assert_eq!(c.read(PCMR_USER, Cycles(0), 0), Some(1));
        assert_eq!(c.read(PCCR_MACHINE, Cycles(7), 0), c.read(PCCR_USER, Cycles(7), 0));
    }

    #[test]
    fn writing_the_counter_sets_its_value_and_counting_resumes_from_there() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        assert!(c.write(PCCR_USER, 1000, Cycles(50), 5));
        assert_eq!(c.read(PCCR_USER, Cycles(60), 6), Some(1010));
    }

    #[test]
    fn disabling_freezes_the_value_reached() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 0, Cycles(30), 3);
        assert_eq!(c.read(PCCR_USER, Cycles(900), 90), Some(30));
    }

    #[test]
    fn instruction_event_counts_retired_instructions() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, PCER_INSTRUCTIONS, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(400), 25), Some(25));
    }

    #[test]
    fn counter_wraps_at_32_bits() {
        let mut c = Esp32C3Csrs::default();
        c.write(PCER_USER, 1, Cycles(0), 0);
        c.write(PCMR_USER, 1, Cycles(0), 0);
        c.write(PCCR_USER, u32::MAX, Cycles(0), 0);
        assert_eq!(c.read(PCCR_USER, Cycles(2), 0), Some(1));
    }

    #[test]
    fn ustatus_keeps_only_its_two_defined_bits() {
        let mut c = Esp32C3Csrs::default();
        assert!(c.write(USTATUS, 0xffff_ffff, Cycles(0), 0));
        assert_eq!(c.read(USTATUS, Cycles(0), 0), Some(USTATUS_UIE | USTATUS_UPIE));
    }

    #[test]
    fn other_addresses_are_not_vendor_csrs() {
        let mut c = Esp32C3Csrs::default();
        assert_eq!(c.read(0x7E3, Cycles(0), 0), None);
        assert!(!c.write(0x803, 1, Cycles(0), 0));
    }
}
```

- [ ] **Step 2: Écrire les tests au niveau du hart (`hart.rs`, module `tests`)**

```rust
    fn c3_machine(program: &[u32]) -> (Hart, Ram) {
        let mut ram = Ram::new(0, 0x1000);
        for (i, word) in program.iter().enumerate() {
            ram.load(4 * u32::try_from(i).unwrap(), &word.to_le_bytes()).unwrap();
        }
        let config = HartConfig { vendor_csrs: VendorCsrs::Esp32C3, ..HartConfig::default() };
        (Hart::new(0, config), ram)
    }

    #[test]
    fn esp32c3_cycle_counter_advances_between_two_reads() {
        // The ROM's ets_delay_us enables the counter through 0x800/0x801 and polls 0x802.
        let (mut hart, mut ram) = c3_machine(&[
            csrrwi(0, 0x800, 1),
            csrrwi(0, 0x801, 1),
            csrrs(5, 0x802, 0),
            addi(0, 0, 0),
            csrrs(6, 0x802, 0),
        ]);
        run(&mut hart, &mut ram, 5);
        assert_eq!(hart.x[6] - hart.x[5], 2);
    }

    #[test]
    fn vendor_csrs_do_not_exist_on_a_generic_hart() {
        let (mut hart, mut ram) = machine(&[csrrs(5, 0x802, 0)], MisalignedPolicy::Trap);
        let outcome = run(&mut hart, &mut ram, 1);
        assert!(matches!(outcome[0], StepOutcome::Trapped { trap, .. } if trap.cause == Exception::IllegalInstr));
    }

    #[test]
    fn mcycle_write_takes_precedence_over_the_cycles_of_the_writing_instruction() {
        let (mut hart, mut ram) = machine(
            &[addi(5, 0, 100), csrrw(0, MCYCLE, 5), csrrs(6, MCYCLE, 0), csrrs(7, MCYCLE, 0)],
            MisalignedPolicy::Trap,
        );
        run(&mut hart, &mut ram, 4);
        assert_eq!(hart.x[6], 100);
        assert_eq!(hart.x[7], 101);
    }
```

(`machine`, `run` et `OneCycle` existent déjà dans ce module de tests : une instruction coûte 1 cycle.)

- [ ] **Step 3: Lancer, constater l'échec de compilation**

Run: `cargo test -p cpu-rv32 > /tmp/cpu.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 4: Implémenter**

`crates/cpu-rv32/src/csr_esp32c3.rs` :

```rust
//! CSRs specific to the ESP32-C3 core. Source: `docs/research/esp32c3-boot-chain.md`, §B.2.
use emu_core::Cycles;

/// User status. The ROM writes 0 then 1 in `_init`; nothing reads it back.
pub const USTATUS: u16 = 0x000;
/// Performance counter: event select, mode and count, machine addresses (ESP-IDF `rv_utils.h`).
pub const PCER_MACHINE: u16 = 0x7E0;
pub const PCMR_MACHINE: u16 = 0x7E1;
pub const PCCR_MACHINE: u16 = 0x7E2;
/// The same three registers at their user addresses, the ones the ROM uses.
pub const PCER_USER: u16 = 0x800;
pub const PCMR_USER: u16 = 0x801;
pub const PCCR_USER: u16 = 0x802;

/// `ustatus` bits of the RISC-V N extension draft.
/// ASSUMPTION: the silicon keeps only these two; a readback on a real chip would settle it.
pub const USTATUS_UIE: u32 = 1 << 0;
pub const USTATUS_UPIE: u32 = 1 << 4;

/// Event selection: CPU clock cycles (the value the ROM and ESP-IDF write).
pub const PCER_CYCLES: u32 = 1 << 0;
/// Event selection: retired instructions.
/// ASSUMPTION: bit position taken from the ESP32-C3 technical reference manual, not exercised by the ROM.
pub const PCER_INSTRUCTIONS: u32 = 1 << 1;
/// Mode register: global count enable.
pub const PCMR_ENABLE: u32 = 1 << 0;

#[derive(Clone, Debug, Default)]
pub(crate) struct Esp32C3Csrs {
    ustatus: u32,
    pcer: u32,
    pcmr: u32,
    /// Counter value at the snapshot below.
    count: u32,
    snapshot_cycles: u64,
    snapshot_instret: u64,
}

#[allow(clippy::cast_possible_truncation)] // deliberate: the counter is 32 bits wide and wraps
const fn low(v: u64) -> u32 {
    v as u32
}

impl Esp32C3Csrs {
    fn counted(&self, cycles: Cycles, instret: u64) -> u32 {
        if self.pcmr & PCMR_ENABLE == 0 {
            return self.count;
        }
        // Other event selections (hazards, branches, loads…) are not modelled: the counter then stands still.
        let elapsed = if self.pcer & PCER_CYCLES != 0 {
            cycles.get().wrapping_sub(self.snapshot_cycles)
        } else if self.pcer & PCER_INSTRUCTIONS != 0 {
            instret.wrapping_sub(self.snapshot_instret)
        } else {
            0
        };
        self.count.wrapping_add(low(elapsed))
    }

    /// Freezes the value counted so far, so a configuration change only affects what follows.
    fn resnapshot(&mut self, cycles: Cycles, instret: u64) {
        self.count = self.counted(cycles, instret);
        self.snapshot_cycles = cycles.get();
        self.snapshot_instret = instret;
    }

    pub(crate) fn read(&self, addr: u16, cycles: Cycles, instret: u64) -> Option<u32> {
        Some(match addr {
            USTATUS => self.ustatus,
            PCER_MACHINE | PCER_USER => self.pcer,
            PCMR_MACHINE | PCMR_USER => self.pcmr,
            PCCR_MACHINE | PCCR_USER => self.counted(cycles, instret),
            _ => return None,
        })
    }

    pub(crate) fn write(&mut self, addr: u16, value: u32, cycles: Cycles, instret: u64) -> bool {
        match addr {
            USTATUS => self.ustatus = value & (USTATUS_UIE | USTATUS_UPIE),
            PCER_MACHINE | PCER_USER => {
                self.resnapshot(cycles, instret);
                self.pcer = value;
            }
            PCMR_MACHINE | PCMR_USER => {
                self.resnapshot(cycles, instret);
                self.pcmr = value;
            }
            PCCR_MACHINE | PCCR_USER => {
                self.resnapshot(cycles, instret);
                self.count = value;
            }
            _ => return false,
        }
        true
    }
}
```

Dans `csr.rs` :

```rust
/// CSRs that only one vendor's core implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VendorCsrs {
    /// A plain RV32IMC machine-mode hart (conformance tests).
    #[default]
    None,
    Esp32C3,
}
```

- `CsrFile` reçoit deux champs : `vendor: Option<Esp32C3Csrs>` et `cycle_written: bool`. `CsrFile::new(vendor: VendorCsrs) -> CsrFile` construit `Some(Esp32C3Csrs::default())` pour `Esp32C3` ; `Default` reste l'équivalent de `VendorCsrs::None`.
- `read` : le bras final devient `_ => return self.vendor.as_ref().and_then(|v| v.read(addr, cycles, instret)),`.
- `write_with_counters` : le bras final devient `_ => return self.vendor.as_mut().is_some_and(|v| v.write(addr, value, cycles, instret)),`. Les bras `MCYCLE` et `MCYCLEH` posent `self.cycle_written = true`.
- `on_instruction_retired(&mut self, cycles_spent: u32)` : en plus du traitement de `instret_written`, si `std::mem::take(&mut self.cycle_written)` alors `self.cycle_offset = self.cycle_offset.wrapping_sub(u64::from(cycles_spent));`. Mettre à jour le commentaire de doc : une écriture de `mcycle` l'emporte sur les cycles de l'instruction qui l'écrit, comme pour `minstret`.
- `write` (commodité) remet aussi `cycle_written` à `false`.

Dans `hart.rs` :

- `HartConfig` reçoit `pub vendor_csrs: VendorCsrs` ; `Hart::new` utilise `CsrFile::new(config.vendor_csrs)`.
- Dans `step`, `self.csr.on_instruction_retired(cycles);` (après `self.cycles = …`, avec la variable `cycles` déjà calculée).
- `set_interrupt_pending` commence par `debug_assert!(line < 32, "interrupt line {line} out of range");` et son commentaire de doc dit que la ligne doit être inférieure à 32.

Dans `lib.rs` : `mod csr_esp32c3;`, ré-export `pub use csr::VendorCsrs;`, et

```rust
#[cfg(any(test, feature = "test-support"))]
pub mod asm;
```

Dans `Cargo.toml` de `cpu-rv32` :

```toml
[features]
# Exposes the instruction encoders (`asm`) to the tests of other crates.
test-support = []
```

Rendu public, `asm` tombe sous `clippy::pedantic` : ajouter `#[must_use]` aux encodeurs et ce que clippy demande d'autre pour une API publique ; ne rien changer à leur comportement.

Dans `crates/emu-cli/src/main.rs`, le littéral devient `HartConfig { misaligned: …, ..HartConfig::default() }`. Faire de même pour tout autre littéral `HartConfig { … }` du workspace (`/usr/bin/git grep -n "HartConfig {"`).

- [ ] **Step 5: Lancer toute la suite**

Run: `cargo test --workspace > /tmp/ws.log 2>&1; echo "exit=$?"; cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"`
Expected: `exit=0`, `clippy=0`. Les 64 tests `riscv-tests` et le test `minstret` existants passent toujours.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/cpu-rv32 crates/emu-cli/src/main.rs
/usr/bin/git commit -m "feat(cpu-rv32): add the ESP32-C3 vendor CSRs and make mcycle writes win over the increment" -- crates/cpu-rv32 crates/emu-cli/src/main.rs
```

---
### Task 4: `soc-esp32c3` — carte mémoire, révision de puce, image de ROM

**Files:**
- Create: `crates/soc-esp32c3/Cargo.toml`, `crates/soc-esp32c3/src/lib.rs`, `crates/soc-esp32c3/src/memmap.rs`, `crates/soc-esp32c3/src/chip.rs`, `crates/soc-esp32c3/src/rom.rs`, `crates/soc-esp32c3/src/test_support.rs`
- Modify: `Cargo.toml` (membre, `soc-esp32c3` et `svd-regs` dans `[workspace.dependencies]`), `tools/research/elf2rom.py` (aucun changement attendu : sert de contre-épreuve)

**Interfaces:**
- Consumes: `goblin` (déjà dans le workspace).
- Produces:
  ```rust
  // soc_esp32c3::memmap — toutes `pub const … : u32`
  ROM_BASE = 0x4000_0000, ROM_SIZE = 0x6_0000, DROM_BASE = 0x3FF0_0000, DROM_SIZE = 0x2_0000, DROM_ROM_OFFSET = 0x4_0000,
  SRAM_DBUS_BASE = 0x3FC8_0000, SRAM_IBUS_BASE = 0x4038_0000, SRAM_SIZE = 0x6_0000,
  SRAM0_BASE = 0x4037_C000, SRAM0_SIZE = 0x4000, RTC_RAM_BASE = 0x5000_0000, RTC_RAM_SIZE = 0x2000,
  PERIPH_BASE = 0x6000_0000, PERIPH_SIZE = 0x10_0000, RESET_PC = ROM_BASE
  // soc_esp32c3::chip
  #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
  pub struct ChipRevision { /* privé */ }
  impl ChipRevision { pub const V0_3: ChipRevision; pub fn new(major: u8, minor: u8) -> Result<ChipRevision, ChipRevisionError>;
                      pub fn major(self) -> u8; pub fn minor(self) -> u8; pub fn full(self) -> u16 /* major*100+minor */ }
  impl FromStr for ChipRevision  // "v0.3"
  impl Display for ChipRevision  // "v0.3"
  // soc_esp32c3::rom
  pub struct RomImage { /* privé */ }
  impl RomImage { pub fn from_elf(file: &[u8]) -> Result<RomImage, RomError>;
                  pub fn bytes(&self) -> &[u8];          // exactement ROM_SIZE octets
                  pub fn eco_version(&self) -> u32;
                  #[cfg(any(test, feature = "test-support"))] pub fn from_flat(bytes: Vec<u8>) -> Result<RomImage, RomError>; }
  pub enum RomError { NotElf, WrongMachine, WrongEntry, WrongChip, MissingSymbol, Truncated, UnexpectedLayout, WrongSize }
  // soc_esp32c3::test_support — compilé sous cfg(any(test, feature = "test-support"))
  pub struct ElfBuilder { pub machine: u16, pub entry: u32, pub segments: Vec<(u32, u32, Vec<u8>)>,
                          pub sections: Vec<(&'static str, u32, Vec<u8>)>, pub symbols: Vec<(&'static str, u32)> }
  impl ElfBuilder { pub fn build(&self) -> Vec<u8> }
  pub fn rom_elf() -> ElfBuilder;                  // un ELF de ROM C3 valide, en miniature
  pub fn flat_rom(program: &[u32]) -> RomImage;   // `program` placé au vecteur de reset, le reste à zéro
  ```

Recette de conversion et sa validation : `docs/research/esp32c3-rom-elfs.md`, §3, §7 et §9. Carte mémoire : `docs/research/esp32c3-boot-chain.md`, §A.1. Champs de révision : même rapport, §H.

- [ ] **Step 1: Créer la crate**

`crates/soc-esp32c3/Cargo.toml` :

```toml
[package]
name = "soc-esp32c3"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[features]
# Exposes `RomImage::from_flat` so other crates can run tiny hand-written programs as a ROM in their tests.
test-support = []

[dependencies]
emu-core.workspace = true
cpu-rv32.workspace = true
svd-regs.workspace = true
goblin.workspace = true
thiserror.workspace = true

[dev-dependencies]
cpu-rv32 = { workspace = true, features = ["test-support"] }
sha2.workspace = true
```

`lib.rs` commence par `#![cfg_attr(test, allow(clippy::unwrap_used))]`, comme `cpu-rv32`.

- [ ] **Step 2: Écrire les tests de `chip.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_the_v_major_dot_minor_form() {
        let rev: ChipRevision = "v0.3".parse().unwrap();
        assert_eq!((rev.major(), rev.minor(), rev.full()), (0, 3, 3));
        assert_eq!(rev.to_string(), "v0.3");
        assert_eq!("v1.1".parse::<ChipRevision>().unwrap().full(), 101);
        assert_eq!(rev, ChipRevision::V0_3);
    }

    #[test]
    fn rejects_values_the_efuse_fields_cannot_hold() {
        assert!(ChipRevision::new(4, 0).is_err()); // WAFER_VERSION_MAJOR is 2 bits
        assert!(ChipRevision::new(0, 16).is_err()); // WAFER_VERSION_MINOR is 4 bits
        assert!(ChipRevision::new(3, 15).is_ok());
    }

    #[test]
    fn malformed_text_names_the_expected_form() {
        for text in ["0.3", "v3", "v0.x", "", "v0.3.1", "v-1.0"] {
            let err = text.parse::<ChipRevision>().unwrap_err().to_string();
            assert!(err.contains("v<major>.<minor>"), "{text:?}: {err}");
        }
    }

    #[test]
    fn revisions_order_by_major_then_minor() {
        assert!("v0.4".parse::<ChipRevision>().unwrap() < "v1.1".parse::<ChipRevision>().unwrap());
    }
}
```

- [ ] **Step 3: Écrire le constructeur d'ELF de test et les tests de `rom.rs`**

Le bloc ci-dessous montre le constructeur et les tests à la suite, pour la lisibilité. Les répartir ainsi :

- `src/test_support.rs`, déclaré dans `lib.rs` par `#[cfg(any(test, feature = "test-support"))] pub mod test_support;` : `ElfBuilder` (champs `pub`, méthode `build` `pub`), ses fonctions auxiliaires, les constantes `RISCV`, `CODE_LEN`, `UNPACK_TABLE`, la fonction `pub fn rom_elf()`, et en plus
  ```rust
  /// A ROM holding `program` at the reset vector and zeros elsewhere.
  pub fn flat_rom(program: &[u32]) -> RomImage {
      let mut rom = vec![0u8; ROM_SIZE as usize];
      for (i, word) in program.iter().enumerate() {
          rom[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
      }
      RomImage::from_flat(rom).unwrap()
  }
  ```
  Ce module est du code de test compilé hors `cfg(test)` quand la feature est active : le commencer par `#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]` avec le commentaire « test fixtures: a panic is a test failure ».
- `src/rom.rs`, module `tests` : uniquement les fonctions `#[test]`, avec `use crate::test_support::{ElfBuilder, rom_elf};`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memmap::ROM_BASE;

    /// A minimal ELF32 little-endian writer: enough structure for `RomImage::from_elf`, nothing more.
    #[derive(Default)]
    struct ElfBuilder {
        machine: u16,
        entry: u32,
        segments: Vec<(u32, u32, Vec<u8>)>, // (vaddr, paddr, bytes)
        sections: Vec<(&'static str, u32, Vec<u8>)>, // (name, addr, bytes): PROGBITS, not ALLOC
        symbols: Vec<(&'static str, u32)>,
    }

    fn u16le(out: &mut Vec<u8>, v: u16) { out.extend_from_slice(&v.to_le_bytes()); }
    fn u32le(out: &mut Vec<u8>, v: u32) { out.extend_from_slice(&v.to_le_bytes()); }
    fn len32(bytes: &[u8]) -> u32 { u32::try_from(bytes.len()).unwrap() }

    impl ElfBuilder {
        fn build(&self) -> Vec<u8> {
            const EHDR: u32 = 52;
            const PHDR: u32 = 32;
            let mut body = Vec::new(); // everything after the program headers
            let body_base = EHDR + PHDR * len32_of(self.segments.len());
            let mut place = |bytes: &[u8]| { let off = body_base + len32(&body); body.extend_from_slice(bytes); off };
            let seg_offs: Vec<u32> = self.segments.iter().map(|s| place(&s.2)).collect();
            let sec_offs: Vec<u32> = self.sections.iter().map(|s| place(&s.2)).collect();
            // String tables: section names then symbol names.
            let mut shstr = vec![0u8];
            let mut name_off = |s: &str| { let o = len32(&shstr); shstr.extend_from_slice(s.as_bytes()); shstr.push(0); o };
            let sec_names: Vec<u32> = self.sections.iter().map(|s| name_off(s.0)).collect();
            let (n_shstr, n_strtab, n_symtab) = (name_off(".shstrtab"), name_off(".strtab"), name_off(".symtab"));
            let mut strtab = vec![0u8];
            let mut symtab = vec![0u8; 16]; // null symbol
            for (name, value) in &self.symbols {
                u32le(&mut symtab, len32(&strtab));
                strtab.extend_from_slice(name.as_bytes());
                strtab.push(0);
                u32le(&mut symtab, *value);
                u32le(&mut symtab, 0);
                symtab.extend_from_slice(&[0x10, 0, 0xf1, 0xff]); // GLOBAL NOTYPE, SHN_ABS
            }
            let (shstr_off, strtab_off, symtab_off) = (place(&shstr), place(&strtab), place(&symtab));
            let shoff = body_base + len32(&body);
            let user = len32_of(self.sections.len());
            let (shstr_idx, strtab_idx) = (user + 1, user + 2);
            let mut shdrs = vec![0u8; 40]; // null section
            let mut shdr = |name: u32, kind: u32, addr: u32, off: u32, size: u32, link: u32, info: u32, entsize: u32| {
                for v in [name, kind, 0, addr, off, size, link, info, 1, entsize] { u32le(&mut shdrs, v); }
            };
            for (i, s) in self.sections.iter().enumerate() {
                shdr(sec_names[i], 1, s.1, sec_offs[i], len32(&s.2), 0, 0, 0);
            }
            shdr(n_shstr, 3, 0, shstr_off, len32(&shstr), 0, 0, 0);
            shdr(n_strtab, 3, 0, strtab_off, len32(&strtab), 0, 0, 0);
            shdr(n_symtab, 2, 0, symtab_off, len32(&symtab), strtab_idx, 1, 16);

            let mut out = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            u16le(&mut out, 2); // ET_EXEC
            u16le(&mut out, self.machine);
            u32le(&mut out, 1);
            u32le(&mut out, self.entry);
            u32le(&mut out, EHDR); // e_phoff
            u32le(&mut out, shoff);
            u32le(&mut out, 1); // e_flags: RVC
            for v in [52u16, 32, u16::try_from(self.segments.len()).unwrap(), 40, u16::try_from(user + 4).unwrap(), u16::try_from(shstr_idx).unwrap()] {
                u16le(&mut out, v);
            }
            for (i, (vaddr, paddr, bytes)) in self.segments.iter().enumerate() {
                for v in [1, seg_offs[i], *vaddr, *paddr, len32(bytes), len32(bytes), 5, 4] { u32le(&mut out, v); }
            }
            out.extend_from_slice(&body);
            out.extend_from_slice(&shdrs);
            out
        }
    }

    fn len32_of(n: usize) -> u32 { u32::try_from(n).unwrap() }

    const RISCV: u16 = 243;
    const CODE_LEN: usize = 0x200;
    const UNPACK_TABLE: usize = 0x100;

    /// A ROM laid out like the real ones, in miniature:
    /// code at 0x4000_0000 (0x200 bytes, zero-filled segment up to 0x300), an unpack table with two
    /// entries whose sources are 0x300 and 0x308, a 4-byte hole at 0x304 for `.data_btdm`.
    fn rom_elf() -> ElfBuilder {
        let mut code = vec![0u8; CODE_LEN];
        code[0x10..0x14].copy_from_slice(&5u32.to_le_bytes()); // _rom_chip_id
        code[0x14..0x18].copy_from_slice(&3u32.to_le_bytes()); // _rom_eco_version
        let table: [u32; 8] = [0x3FCD_F000, 0x3FCD_F004, 0x4000_0300, 0, 0x3FCD_F010, 0x3FCD_F014, 0x4000_0308, 0];
        for (i, word) in table.iter().enumerate() {
            code[UNPACK_TABLE + 4 * i..UNPACK_TABLE + 4 * i + 4].copy_from_slice(&word.to_le_bytes());
        }
        ElfBuilder {
            machine: RISCV,
            entry: ROM_BASE,
            segments: vec![
                (0x3FCC_9000, 0x3FCC_A000, b"\x7fELF".to_vec()), // linker artefact: must be ignored
                (ROM_BASE, ROM_BASE, code),
                (0x3FCC_AE00, ROM_BASE + 0x200, vec![0; 0x100]), // zeroed init-data placeholder
            ],
            sections: vec![
                (".data_x", 0x3FCD_F000, vec![1, 2, 3, 4]),
                (".data_y", 0x3FCD_F010, vec![5, 6, 7, 8]),
                (".data_btdm", 0x3FCD_F020, vec![9, 9, 9, 9]),
                (".rodata.interface", 0x3FF1_FFFC, vec![0xAA, 0xBB, 0xCC, 0xDD]),
            ],
            symbols: vec![
                ("_rom_chip_id", ROM_BASE + 0x10),
                ("_rom_eco_version", ROM_BASE + 0x14),
                ("_data_start", ROM_BASE + 0x100),
                ("_data_end", ROM_BASE + 0x120),
                ("_bss_end", ROM_BASE + 0x200),
            ],
        }
    }

    #[test]
    fn converts_segments_sections_and_initial_data() {
        let rom = RomImage::from_elf(&rom_elf().build()).unwrap();
        let b = rom.bytes();
        assert_eq!(b.len(), 0x6_0000);
        assert_eq!(rom.eco_version(), 3);
        assert_eq!(&b[0x300..0x304], [1, 2, 3, 4], "unpack entry 0");
        assert_eq!(&b[0x308..0x30C], [5, 6, 7, 8], "unpack entry 1");
        assert_eq!(&b[0x304..0x308], [9, 9, 9, 9], ".data_btdm fills the hole between them");
        assert_eq!(&b[0x5_FFFC..], [0xAA, 0xBB, 0xCC, 0xDD], "non-ALLOC section in the DROM window");
        assert!(b[0x30C..0x4_0000].iter().all(|&x| x == 0), "unprovided bytes stay zero");
    }

    #[test]
    fn rejects_non_riscv_elf() {
        let mut elf = rom_elf();
        elf.machine = 94; // Xtensa: an ESP32-S3 ROM
        let err = RomImage::from_elf(&elf.build()).unwrap_err();
        assert!(matches!(err, RomError::WrongMachine { machine: 94 }), "{err}");
        assert!(err.to_string().contains("esp32c3_rev"), "{err}");
    }

    #[test]
    fn rejects_wrong_chip_id() {
        let mut elf = rom_elf();
        elf.segments[1].2[0x10] = 13;
        assert!(matches!(RomImage::from_elf(&elf.build()), Err(RomError::WrongChip { found: 13 })));
    }

    #[test]
    fn rejects_an_entry_point_outside_the_reset_vector() {
        let mut elf = rom_elf();
        elf.entry = 0x4000_0400;
        assert!(matches!(RomImage::from_elf(&elf.build()), Err(RomError::WrongEntry { .. })));
    }

    #[test]
    fn missing_symbol_is_named() {
        let mut elf = rom_elf();
        elf.symbols.retain(|s| s.0 != "_data_end");
        let err = RomImage::from_elf(&elf.build()).unwrap_err();
        assert!(matches!(err, RomError::MissingSymbol { name: "_data_end" }), "{err}");
    }

    #[test]
    fn btdm_section_that_does_not_fit_its_hole_is_a_layout_error() {
        let mut elf = rom_elf();
        elf.sections[2].2 = vec![9; 8]; // the hole is 4 bytes
        assert!(matches!(RomImage::from_elf(&elf.build()), Err(RomError::UnexpectedLayout { .. })));
    }

    #[test]
    fn not_an_elf_is_an_error_with_a_remedy() {
        let err = RomImage::from_elf(b"ESP-ROM, honest").unwrap_err();
        assert!(matches!(err, RomError::NotElf { .. }));
        assert!(err.to_string().contains("esp-rom-elfs"), "{err}");
    }

    #[test]
    fn truncated_elf_is_an_error_not_a_panic() {
        let full = rom_elf().build();
        for cut in [0, 10, 52, 60, full.len() / 2, full.len() - 1] {
            assert!(RomImage::from_elf(&full[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn flat_image_must_be_exactly_the_rom_size() {
        assert!(RomImage::from_flat(vec![0; 0x6_0000]).is_ok());
        assert!(matches!(RomImage::from_flat(vec![0; 0x5_FFFF]), Err(RomError::WrongSize { len: 0x5_FFFF })));
    }
}
```

- [ ] **Step 4: Lancer, constater l'échec de compilation**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 5: Implémenter `memmap.rs`, `chip.rs`, `rom.rs`**

`memmap.rs` : les constantes de la section Interfaces, chacune avec sa source en commentaire (`ESP-IDF soc.h SOC_IROM_MASK_LOW`, etc., reprises du rapport §A.1).

`chip.rs` :

```rust
//! Silicon revision of the chip, as the eFuse wafer-version fields encode it.
use std::fmt;
use std::str::FromStr;

/// `WAFER_VERSION_MAJOR` is 2 bits wide, `WAFER_VERSION_MINOR` 4 (3 low bits + 1 high bit).
const MAX_MAJOR: u8 = 3;
const MAX_MINOR: u8 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChipRevision {
    major: u8,
    minor: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChipRevisionError {
    #[error("chip revision v{major}.{minor} does not fit the eFuse fields (major 0..=3, minor 0..=15); use a revision Espressif shipped, such as v0.3, v0.4 or v1.1")]
    OutOfRange { major: u8, minor: u8 },
    #[error("`{text}` is not a chip revision; write it as v<major>.<minor>, for example v0.3")]
    Malformed { text: String },
}

impl ChipRevision {
    pub const V0_3: ChipRevision = ChipRevision { major: 0, minor: 3 };

    /// # Errors
    /// [`ChipRevisionError::OutOfRange`] when a value does not fit its eFuse field.
    pub fn new(major: u8, minor: u8) -> Result<ChipRevision, ChipRevisionError> {
        if major > MAX_MAJOR || minor > MAX_MINOR {
            return Err(ChipRevisionError::OutOfRange { major, minor });
        }
        Ok(ChipRevision { major, minor })
    }
    #[must_use]
    pub const fn major(self) -> u8 { self.major }
    #[must_use]
    pub const fn minor(self) -> u8 { self.minor }
    /// ESP-IDF's `efuse_hal_chip_revision`: `major * 100 + minor`.
    #[must_use]
    pub fn full(self) -> u16 { u16::from(self.major) * 100 + u16::from(self.minor) }
}

impl FromStr for ChipRevision {
    type Err = ChipRevisionError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let malformed = || ChipRevisionError::Malformed { text: text.to_owned() };
        let (major, minor) = text.strip_prefix('v').and_then(|rest| rest.split_once('.')).ok_or_else(malformed)?;
        let major = major.parse::<u8>().map_err(|_| malformed())?;
        let minor = minor.parse::<u8>().map_err(|_| malformed())?;
        ChipRevision::new(major, minor)
    }
}

impl fmt::Display for ChipRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}.{}", self.major, self.minor)
    }
}
```

`rom.rs` :

```rust
//! Builds the 384 KiB mask-ROM image from an Espressif ROM ELF.
//! Recipe and byte-for-byte validation against a reference image: `docs/research/esp32c3-rom-elfs.md`, §7 and §9.
use crate::memmap::{DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, ROM_BASE, ROM_SIZE};
use goblin::elf::Elf;
use goblin::elf::header::EM_RISCV;
use goblin::elf::program_header::PT_LOAD;
use goblin::elf::section_header::{SHT_PROGBITS, SectionHeader};
use std::ops::Range;

/// `_rom_chip_id` of an ESP32-C3 ROM (`esp32c3.rom.version.ld`).
const ESP32C3_CHIP_ID: u32 = 5;
/// One `unpack` entry: destination start, destination end, source, reserved.
const UNPACK_ENTRY_BYTES: u64 = 16;
/// The real tables hold 31 to 33 entries; this bounds the loop on a hostile file.
const MAX_UNPACK_TABLE_BYTES: u64 = 0x1000;
const BTDM_DATA_SECTION: &str = ".data_btdm";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RomError {
    #[error("not an ELF file ({detail}); pass the `esp32c3_rev<N>_rom.elf` file from the esp-rom-elfs archive")]
    NotElf { detail: String },
    #[error("ELF machine {machine} is not 32-bit little-endian RISC-V; use an ESP32-C3 ROM (`esp32c3_rev<N>_rom.elf`), not the ROM of another chip")]
    WrongMachine { machine: u16 },
    #[error("ELF entry point {entry:#010x} is not the reset vector 0x40000000; use an unmodified ESP32-C3 ROM ELF")]
    WrongEntry { entry: u64 },
    #[error("the ROM identifies itself as chip {found}, not 5 (ESP32-C3); use an `esp32c3_rev<N>_rom.elf` file")]
    WrongChip { found: u32 },
    #[error("the ELF has no `{name}` symbol; use an unstripped ROM ELF from the esp-rom-elfs archive")]
    MissingSymbol { name: &'static str },
    #[error("the ELF is truncated: {what} lies beyond the end of the file; download the archive again")]
    Truncated { what: &'static str },
    #[error("unexpected ROM layout: {detail}; this ROM revision needs a review of the conversion recipe")]
    UnexpectedLayout { detail: String },
    #[error("a flat ROM image must be exactly 393216 bytes, got {len}; pass a complete 384 KiB dump")]
    WrongSize { len: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RomImage {
    bytes: Box<[u8]>,
    eco_version: u32,
}

/// Offsets inside the ROM of `[addr, addr + len)`, when that range lies entirely in the ROM.
fn rom_range(addr: u64, len: u64) -> Option<Range<usize>> {
    let start = addr.checked_sub(u64::from(ROM_BASE))?;
    let end = start.checked_add(len)?;
    (end <= u64::from(ROM_SIZE)).then(|| usize::try_from(start).ok().zip(usize::try_from(end).ok()))?.map(|(s, e)| s..e)
}

fn file_range<'a>(file: &'a [u8], offset: u64, len: u64, what: &'static str) -> Result<&'a [u8], RomError> {
    let start = usize::try_from(offset).map_err(|_| RomError::Truncated { what })?;
    let len = usize::try_from(len).map_err(|_| RomError::Truncated { what })?;
    start.checked_add(len).and_then(|end| file.get(start..end)).ok_or(RomError::Truncated { what })
}

struct Layout {
    rom: Vec<u8>,
    /// Bytes provided by the ELF; the rest stays zero (alignment padding, validated in §9 of the report).
    covered: Vec<bool>,
}

impl Layout {
    fn put(&mut self, range: Range<usize>, data: &[u8]) {
        // INVARIANT: every caller gets `range` from `rom_range` with `data.len()` as its length.
        self.rom[range.clone()].copy_from_slice(data);
        self.covered[range].fill(true);
    }

    fn word(&self, addr: u64, what: &'static str) -> Result<u32, RomError> {
        let range = rom_range(addr, 4).ok_or_else(|| RomError::UnexpectedLayout { detail: format!("{what} at {addr:#010x} is outside the ROM") })?;
        let mut le = [0u8; 4];
        le.copy_from_slice(&self.rom[range]);
        Ok(u32::from_le_bytes(le))
    }
}

fn symbol(elf: &Elf<'_>, name: &'static str) -> Result<u64, RomError> {
    elf.syms
        .iter()
        .find(|sym| elf.strtab.get_at(sym.st_name) == Some(name))
        .map(|sym| sym.st_value)
        .ok_or(RomError::MissingSymbol { name })
}

fn progbits<'a>(elf: &'a Elf<'_>) -> impl Iterator<Item = &'a SectionHeader> {
    elf.section_headers.iter().filter(|sh| sh.sh_type == SHT_PROGBITS && sh.sh_size > 0)
}

impl RomImage {
    /// # Errors
    /// [`RomError`] when the file is not an unmodified ESP32-C3 ROM ELF.
    pub fn from_elf(file: &[u8]) -> Result<RomImage, RomError> {
        let elf = Elf::parse(file).map_err(|e| RomError::NotElf { detail: e.to_string() })?;
        if elf.is_64 || !elf.little_endian || elf.header.e_machine != EM_RISCV {
            return Err(RomError::WrongMachine { machine: elf.header.e_machine });
        }
        if elf.entry != u64::from(ROM_BASE) {
            return Err(RomError::WrongEntry { entry: elf.entry });
        }
        let size = ROM_SIZE as usize;
        let mut layout = Layout { rom: vec![0; size], covered: vec![false; size] };
        let loads = || elf.program_headers.iter().filter(|ph| ph.p_type == PT_LOAD && ph.p_filesz > 0);

        // 1. Loadable segments whose LOAD address is in the ROM. This drops the linker artefact that
        //    carries the ELF header itself (LOAD address in RAM).
        for ph in loads() {
            if let Some(range) = rom_range(ph.p_paddr, ph.p_filesz) {
                layout.put(range, file_range(file, ph.p_offset, ph.p_filesz, "a loadable segment")?);
            }
        }
        // 2. Sections in the data-bus window that no segment carries (`.rodata.interface` is not ALLOC).
        let drom = u64::from(DROM_BASE)..u64::from(DROM_BASE) + u64::from(DROM_SIZE);
        for sh in progbits(&elf).filter(|sh| drom.contains(&sh.sh_addr)) {
            if loads().any(|ph| (ph.p_vaddr..ph.p_vaddr + ph.p_filesz).contains(&sh.sh_addr)) {
                continue;
            }
            let rom_addr = sh.sh_addr - u64::from(DROM_BASE) + u64::from(DROM_ROM_OFFSET) + u64::from(ROM_BASE);
            let range = rom_range(rom_addr, sh.sh_size).ok_or_else(|| RomError::UnexpectedLayout {
                detail: format!("section at {:#010x} overruns the data-bus window", sh.sh_addr),
            })?;
            layout.put(range, file_range(file, sh.sh_offset, sh.sh_size, "a read-only data section")?);
        }
        // 3. Initial values of `.data_*`: `_init` copies them out of the ROM through the `unpack` table,
        //    but the segments only hold zeros there. The values are in sections at their RAM address.
        let (table, table_end) = (symbol(&elf, "_data_start")?, symbol(&elf, "_data_end")?);
        let table_len = table_end.checked_sub(table).filter(|len| *len <= MAX_UNPACK_TABLE_BYTES && len % UNPACK_ENTRY_BYTES == 0);
        let Some(table_len) = table_len else {
            return Err(RomError::UnexpectedLayout { detail: format!("unpack table {table:#010x}..{table_end:#010x} is malformed") });
        };
        for entry in (0..table_len / UNPACK_ENTRY_BYTES).map(|i| table + i * UNPACK_ENTRY_BYTES) {
            let dst = u64::from(layout.word(entry, "an unpack entry")?);
            let dst_end = u64::from(layout.word(entry + 4, "an unpack entry")?);
            let src = u64::from(layout.word(entry + 8, "an unpack entry")?);
            let Some(len) = dst_end.checked_sub(dst).filter(|len| *len > 0) else { continue };
            let section = progbits(&elf)
                .find(|sh| sh.sh_addr <= dst && dst_end <= sh.sh_addr + sh.sh_size)
                .ok_or_else(|| RomError::UnexpectedLayout { detail: format!("no section holds the initial data for {dst:#010x}..{dst_end:#010x}") })?;
            let data = file_range(file, section.sh_offset + (dst - section.sh_addr), len, "an initial-data section")?;
            let range = rom_range(src, len).ok_or_else(|| RomError::UnexpectedLayout {
                detail: format!("unpack source {src:#010x} is outside the ROM"),
            })?;
            layout.put(range, data);
        }
        // 4. `.data_btdm` has no unpack entry (the Bluetooth controller copies it itself). In every C3
        //    revision it is the first range after `_bss_end` that nothing else provides, exactly its size.
        if let Some(btdm) = progbits(&elf).find(|sh| elf.shdr_strtab.get_at(sh.sh_name) == Some(BTDM_DATA_SECTION)) {
            let from = rom_range(symbol(&elf, "_bss_end")?, 0).map_or(size, |r| r.start);
            let start = (from..size).find(|&i| !layout.covered[i]).unwrap_or(size);
            let end = (start..size).find(|&i| layout.covered[i]).unwrap_or(size);
            if u64::try_from(end - start).ok() != Some(btdm.sh_size) {
                return Err(RomError::UnexpectedLayout {
                    detail: format!("the hole after _bss_end is {} bytes but {BTDM_DATA_SECTION} is {}", end - start, btdm.sh_size),
                });
            }
            layout.put(start..end, file_range(file, btdm.sh_offset, btdm.sh_size, BTDM_DATA_SECTION)?);
        }
        // 5. Identity.
        let found = layout.word(symbol(&elf, "_rom_chip_id")?, "_rom_chip_id")?;
        if found != ESP32C3_CHIP_ID {
            return Err(RomError::WrongChip { found });
        }
        let eco_version = layout.word(symbol(&elf, "_rom_eco_version")?, "_rom_eco_version")?;
        Ok(RomImage { bytes: layout.rom.into_boxed_slice(), eco_version })
    }

    /// A raw image, for tests that run a hand-written program as the ROM.
    /// # Errors
    /// [`RomError::WrongSize`] unless `bytes` is exactly the ROM size.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_flat(bytes: Vec<u8>) -> Result<RomImage, RomError> {
        if bytes.len() != ROM_SIZE as usize {
            return Err(RomError::WrongSize { len: bytes.len() });
        }
        let mut eco = [0u8; 4];
        eco.copy_from_slice(&bytes[0x14..0x18]);
        Ok(RomImage { bytes: bytes.into_boxed_slice(), eco_version: u32::from_le_bytes(eco) })
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] { &self.bytes }
    /// `_rom_eco_version`: 0, 3 or 7 for the three published ESP32-C3 ROMs.
    #[must_use]
    pub const fn eco_version(&self) -> u32 { self.eco_version }
}
```

Le code ci-dessus est la référence de comportement ; si `goblin` expose un nom différent pour un champ (`shdr_strtab`, `strtab`, `syms`), utiliser l'équivalent de la version installée sans changer l'algorithme. `rom_range` peut être récrit plus lisiblement tant que les tests passent. Si `WrongSize` n'est construit que sous `cfg(test)`, garder la variante publique (elle fait partie de l'API de `test-support`) et annoter le besoin, pas `#[allow(dead_code)]` global.

- [ ] **Step 6: Lancer les tests**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=0` (4 tests `chip`, 9 tests `rom`).

- [ ] **Step 7: Écrire le test sur les vraies ROM (`crates/soc-esp32c3/tests/real_rom.rs`)**

```rust
//! Tests against the real mask ROMs. They need the ELF files of the pinned esp-rom-elfs archive:
//! `INKEMU_ROM_ELF_DIR=<dir> cargo test -p soc-esp32c3 --test real_rom -- --ignored`
use sha2::{Digest, Sha256};
use soc_esp32c3::rom::RomImage;

fn elf(revision: u16) -> Vec<u8> {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR to the directory holding the ROM ELF files");
    let path = std::path::Path::new(&dir).join(format!("esp32c3_rev{revision}_rom.elf"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn image_sha256(revision: u16) -> String {
    let rom = RomImage::from_elf(&elf(revision)).unwrap();
    Sha256::digest(rom.bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// rev3 is byte-identical to the reference ROM image shipped with Espressif's QEMU; rev0 and rev101
/// have no reference image and are pinned to what the same recipe produces
/// (`docs/research/esp32c3-rom-elfs.md`, §9).
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn converted_images_match_their_pinned_hashes() {
    assert_eq!(image_sha256(3), "0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99");
    assert_eq!(image_sha256(0), "832b9a17b4bca5fe307d2d84b3fcd9cdc75f6a8a51956a3e114b4d6713710565");
    assert_eq!(image_sha256(101), "8b41b9b114e110e373a58188e6e86b044719f3b4e6da0441c6e8b68b72986651");
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn eco_versions_identify_the_three_revisions() {
    let eco = |rev| RomImage::from_elf(&elf(rev)).unwrap().eco_version();
    assert_eq!((eco(0), eco(3), eco(101)), (0, 3, 7));
}
```

Run: `INKEMU_ROM_ELF_DIR=<répertoire des ELF> cargo test -p soc-esp32c3 --test real_rom -- --ignored > /tmp/real.log 2>&1; echo "exit=$?"`
Expected: `exit=0`, 2 tests. Contre-épreuve indépendante : `python3 -I tools/research/elf2rom.py <elf>` affiche la même empreinte `rom384` pour chaque révision. Si une empreinte diffère, **ne pas modifier la constante** : trouver l'écart d'algorithme.

- [ ] **Step 8: Commit**

```bash
/usr/bin/git add crates/soc-esp32c3 Cargo.toml Cargo.lock
/usr/bin/git commit -m "feat(soc-esp32c3): add the memory map constants, chip revision and ROM image conversion" -- crates/soc-esp32c3 Cargo.toml Cargo.lock
```

---

### Task 5: Bus du SoC, répartition MMIO et diagnostics

**Files:**
- Create: `crates/emu-core/src/peripheral.rs`, `crates/soc-esp32c3/src/config.rs`, `crates/soc-esp32c3/src/diag.rs`, `crates/soc-esp32c3/src/bus.rs`, `crates/soc-esp32c3/src/periph/mod.rs`, `crates/soc-esp32c3/src/periph/gpio.rs`, `crates/soc-esp32c3/src/periph/io_mux.rs`
- Modify: `crates/emu-core/src/lib.rs`, `crates/soc-esp32c3/src/lib.rs`

**Interfaces:**
- Consumes: `RomImage`, `memmap::*`, `ChipRevision` (Task 4) ; `svd_regs::esp32c3::{lookup, registers}` (Task 2) ; `emu_core::{Bus, Ram, AccessSize, AccessKind, ReadOutcome, BusError}`.
- Produces:
  ```rust
  // emu_core
  pub trait Peripheral: std::fmt::Debug {
      fn read(&mut self, offset: u32) -> Option<u32>;       // registre de 32 bits à `offset` (multiple de 4) ; None = non modélisé
      fn write(&mut self, offset: u32, value: u32) -> bool; // false = non modélisé
  }
  // soc_esp32c3::config
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub struct SocConfig { pub chip_revision: ChipRevision, pub strapping: u32, pub strict_mmio: bool, pub trace_hash: bool }
  pub const STRAP_SPI_FAST_FLASH_BOOT: u32 = 0x8;
  impl Default for SocConfig  // v0.3, STRAP_SPI_FAST_FLASH_BOOT, false, false
  // soc_esp32c3::diag
  pub enum Direction { Read, Write }
  pub enum Diagnostic {
      UnmodeledMmio { addr: u32, direction: Direction, pc: u32, register: Option<(&'static str, &'static str)>, value: u32, reset_explicit: bool },
      SubWordWrite { addr: u32, bytes: u32, pc: u32 },
      Exception { cause: cpu_rv32::Exception, tval: u32, pc: u32 },
      Suppressed,
  }
  impl Display for Diagnostic
  pub const MAX_DISTINCT_DIAGNOSTICS: usize = 4096;
  pub struct Diagnostics;  impl Diagnostics { pub fn new(strict: bool) -> Self; pub fn record(&mut self, d: Diagnostic);
      pub fn drain(&mut self) -> Vec<Diagnostic>; pub fn take_violation(&mut self) -> Option<Diagnostic>;
      pub fn occurrences(&self, d: &Diagnostic) -> u64; pub fn distinct(&self) -> usize }
  // soc_esp32c3::periph
  pub struct Storage; impl Storage { pub fn for_peripheral(peripheral: &str, base: u32) -> Storage; pub fn for_offsets(base: u32, offsets: &[u32]) -> Storage;
      pub fn read(&self, offset: u32) -> Option<u32>; pub fn write(&mut self, offset: u32, value: u32) -> bool }
  pub struct Peripherals { pub gpio: Gpio, pub io_mux: IoMux /* les tâches 6 et 7 ajoutent des champs */ }
  impl Peripherals { pub fn new(config: &SocConfig) -> Peripherals }
  // soc_esp32c3::bus
  pub struct SocBus { pub periph: Peripherals, pub diag: Diagnostics, /* privé : rom, sram, sram0, rtc, pc */ }
  impl SocBus { pub fn new(rom: RomImage, config: &SocConfig) -> SocBus; pub fn set_pc(&mut self, pc: u32) }
  impl emu_core::Bus for SocBus
  ```

La spec (§3.2) annonce aussi un `tick` dans le contrat `Peripheral` : il est ajouté en 1b-3, avec les premiers périphériques qui dépendent du temps (décision D7).

- [ ] **Step 1: Écrire les tests de `diag.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn read_at(addr: u32, pc: u32) -> Diagnostic {
        Diagnostic::UnmodeledMmio { addr, direction: Direction::Read, pc, register: Some(("LEDC", "CONF")), value: 7, reset_explicit: true }
    }

    #[test]
    fn repeated_diagnostic_is_listed_once_and_counted() {
        let mut d = Diagnostics::new(false);
        for pc in 0..1000 {
            d.record(read_at(0x6001_9000, pc)); // the PC differs, the register does not
        }
        let listed = d.drain();
        assert_eq!(listed.len(), 1);
        assert_eq!(d.occurrences(&listed[0]), 1000);
        assert!(d.drain().is_empty(), "draining twice lists nothing new");
    }

    #[test]
    fn read_and_write_of_the_same_register_are_two_diagnostics() {
        let mut d = Diagnostics::new(false);
        d.record(read_at(0x6001_9000, 0));
        d.record(Diagnostic::UnmodeledMmio { addr: 0x6001_9000, direction: Direction::Write, pc: 0, register: None, value: 1, reset_explicit: false });
        assert_eq!(d.drain().len(), 2);
    }

    #[test]
    fn distinct_diagnostics_are_capped() {
        let mut d = Diagnostics::new(false);
        for i in 0..u32::try_from(MAX_DISTINCT_DIAGNOSTICS).unwrap() + 10 {
            d.record(Diagnostic::SubWordWrite { addr: 0x6000_0000 + 4 * i, bytes: 1, pc: 0 });
        }
        let listed = d.drain();
        assert_eq!(d.distinct(), MAX_DISTINCT_DIAGNOSTICS);
        assert_eq!(listed.len(), MAX_DISTINCT_DIAGNOSTICS + 1);
        assert_eq!(listed.last(), Some(&Diagnostic::Suppressed));
    }

    #[test]
    fn strict_mode_reports_the_first_mmio_diagnostic_as_a_violation() {
        let mut strict = Diagnostics::new(true);
        strict.record(Diagnostic::Exception { cause: cpu_rv32::Exception::IllegalInstr, tval: 0, pc: 4 });
        assert_eq!(strict.take_violation(), None, "a CPU exception is not an MMIO violation");
        strict.record(read_at(0x6001_9000, 8));
        strict.record(read_at(0x6001_9004, 12));
        assert_eq!(strict.take_violation(), Some(read_at(0x6001_9000, 8)));
        assert_eq!(strict.take_violation(), None);
        let mut lenient = Diagnostics::new(false);
        lenient.record(read_at(0x6001_9000, 8));
        assert_eq!(lenient.take_violation(), None);
    }

    #[test]
    fn messages_name_peripheral_register_address_and_pc() {
        let text = read_at(0x6001_9000, 0x4000_1234).to_string();
        for part in ["LEDC.CONF", "0x60019000", "0x40001234", "0x00000007"] {
            assert!(text.contains(part), "{part} missing from: {text}");
        }
        let undocumented = Diagnostic::UnmodeledMmio { addr: 0x6000_4038, direction: Direction::Read, pc: 0, register: Some(("GPIO", "STRAP")), value: 0, reset_explicit: false };
        assert!(undocumented.to_string().contains("documents no reset value"), "{undocumented}");
        let nowhere = Diagnostic::UnmodeledMmio { addr: 0x600C_5000, direction: Direction::Write, pc: 0, register: None, value: 9, reset_explicit: false };
        assert!(nowhere.to_string().contains("no register is documented"), "{nowhere}");
    }
}
```

- [ ] **Step 2: Écrire les tests de `bus.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{Diagnostic, Direction};
    use crate::memmap::*;
    use emu_core::{AccessKind::{Fetch, Load, Store}, AccessSize::{Byte, Half, Word}, Bus, BusError};

    fn bus_with(config: SocConfig) -> SocBus {
        let mut rom = vec![0u8; ROM_SIZE as usize];
        rom[..4].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        rom[0x4_0000..0x4_0004].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        SocBus::new(RomImage::from_flat(rom).unwrap(), &config)
    }
    fn bus() -> SocBus { bus_with(SocConfig::default()) }
    fn load(bus: &mut SocBus, addr: u32) -> u32 { bus.read(addr, Word, Load).unwrap().value }
    /// A register no SP1b plan models, with a documented non-zero reset value.
    fn unmodeled() -> &'static svd_regs::RegisterInfo {
        svd_regs::esp32c3::registers().iter().find(|r| r.peripheral == "LEDC" && r.reset_explicit && r.reset_value != 0).unwrap()
    }

    #[test]
    fn rom_is_visible_on_the_instruction_bus_and_its_upper_part_on_the_data_bus() {
        let mut b = bus();
        assert_eq!(b.read(ROM_BASE, Word, Fetch).unwrap().value, 0x4433_2211);
        assert_eq!(load(&mut b, ROM_BASE), 0x4433_2211, "_init reads its tables through the instruction bus");
        assert_eq!(load(&mut b, DROM_BASE), 0xDDCC_BBAA, "DROM is the ROM from offset 0x40000");
        assert_eq!(load(&mut b, ROM_BASE + DROM_ROM_OFFSET), 0xDDCC_BBAA);
        assert_eq!(b.read(ROM_BASE + 1, Half, Load).unwrap().value, 0x3322);
    }

    #[test]
    fn store_to_rom_faults() {
        let mut b = bus();
        assert_eq!(b.write(ROM_BASE, Word, 0), Err(BusError::Unmapped { addr: ROM_BASE, kind: Store }));
        assert_eq!(b.write(DROM_BASE, Word, 0), Err(BusError::Unmapped { addr: DROM_BASE, kind: Store }));
    }

    #[test]
    fn sram_is_one_memory_seen_through_two_buses() {
        let mut b = bus();
        b.write(SRAM_DBUS_BASE + 0x100, Word, 0xCAFE_F00D).unwrap();
        assert_eq!(b.read(SRAM_IBUS_BASE + 0x100, Word, Fetch).unwrap().value, 0xCAFE_F00D);
        b.write(SRAM_IBUS_BASE + 0x5_FFFC, Word, 7).unwrap();
        assert_eq!(load(&mut b, SRAM_DBUS_BASE + 0x5_FFFC), 7);
    }

    #[test]
    fn write_through_the_data_bus_invalidates_code_cached_at_the_instruction_bus_address() {
        let mut b = bus();
        let code = SRAM_IBUS_BASE + 0x2000;
        let before = b.page_version(code);
        b.write(SRAM_DBUS_BASE + 0x2000, Word, 0x13).unwrap();
        assert_ne!(b.page_version(code), before);
        assert_eq!(b.page_version(code), b.page_version(SRAM_DBUS_BASE + 0x2000));
        let other = b.page_version(SRAM_IBUS_BASE + 0x4000);
        b.write(SRAM_DBUS_BASE + 0x2004, Word, 0x13).unwrap();
        assert_eq!(b.page_version(SRAM_IBUS_BASE + 0x4000), other, "another page is untouched");
    }

    #[test]
    fn sram0_and_rtc_memory_are_plain_ram() {
        let mut b = bus();
        b.write(SRAM0_BASE, Word, 1).unwrap();
        b.write(RTC_RAM_BASE + RTC_RAM_SIZE - 4, Word, 2).unwrap();
        assert_eq!((load(&mut b, SRAM0_BASE), load(&mut b, RTC_RAM_BASE + RTC_RAM_SIZE - 4)), (1, 2));
    }

    #[test]
    fn addresses_outside_every_region_are_unmapped() {
        let mut b = bus();
        for addr in [0, 0x3C00_0000, 0x4200_0000, SRAM_DBUS_BASE - 4, ROM_BASE + ROM_SIZE, RTC_RAM_BASE + RTC_RAM_SIZE, PERIPH_BASE + PERIPH_SIZE] {
            assert_eq!(b.read(addr, Word, Load), Err(BusError::Unmapped { addr, kind: Load }), "{addr:#x}");
        }
        // An access that starts inside a region and ends outside it is unmapped too.
        assert!(b.read(RTC_RAM_BASE + RTC_RAM_SIZE - 2, Word, Load).is_err());
        assert_eq!(b.read(PERIPH_BASE, Word, Fetch), Err(BusError::Unmapped { addr: PERIPH_BASE, kind: Fetch }), "peripherals hold no code");
    }

    #[test]
    fn modeled_registers_answer_without_a_diagnostic() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x6000_4038), STRAP_SPI_FAST_FLASH_BOOT, "GPIO.STRAP");
        b.write(0x6000_9054, Word, 0x1234).unwrap(); // IO_MUX.GPIO20
        assert_eq!(load(&mut b, 0x6000_9054), 0x1234);
        assert!(b.diag.drain().is_empty());
    }

    #[test]
    fn repeated_access_warns_once_and_counts() {
        let mut b = bus();
        let reg = unmodeled();
        b.set_pc(0x4000_0040);
        for _ in 0..1000 {
            assert_eq!(load(&mut b, reg.addr), reg.reset_value);
        }
        let listed = b.diag.drain();
        assert_eq!(listed, [Diagnostic::UnmodeledMmio {
            addr: reg.addr, direction: Direction::Read, pc: 0x4000_0040,
            register: Some((reg.peripheral, reg.name)), value: reg.reset_value, reset_explicit: true,
        }]);
        assert_eq!(b.diag.occurrences(&listed[0]), 1000);
    }

    #[test]
    fn unmodeled_write_is_ignored_and_reported() {
        let mut b = bus();
        let reg = unmodeled();
        b.write(reg.addr, Word, 0xFFFF_FFFF).unwrap();
        assert_eq!(load(&mut b, reg.addr), reg.reset_value, "the write did not stick");
        assert_eq!(b.diag.drain().len(), 2, "one diagnostic for the write, one for the read");
    }

    #[test]
    fn gap_in_peripheral_window_reads_zero_and_warns() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x600C_5000), 0); // MMU table: not in the SVD, not modelled before SP1b-2
        assert!(matches!(b.diag.drain()[..], [Diagnostic::UnmodeledMmio { register: None, value: 0, .. }]));
    }

    #[test]
    fn sub_word_read_extracts_the_addressed_bytes() {
        let mut b = bus();
        b.write(0x6000_9054, Word, 0xA1B2_C3D4).unwrap();
        assert_eq!(b.read(0x6000_9054, Byte, Load).unwrap().value, 0xD4);
        assert_eq!(b.read(0x6000_9055, Byte, Load).unwrap().value, 0xC3);
        assert_eq!(b.read(0x6000_9056, Half, Load).unwrap().value, 0xA1B2);
    }

    #[test]
    fn sub_word_write_is_reported() {
        let mut b = bus();
        b.set_pc(0x4000_0100);
        b.write(0x6000_9054, Word, 0xFFFF_FFFF).unwrap();
        b.write(0x6000_9055, Byte, 0x7E).unwrap();
        assert_eq!(load(&mut b, 0x6000_9054), 0x0000_7E00, "the whole register is written, other bytes as zero");
        assert_eq!(b.diag.drain(), [Diagnostic::SubWordWrite { addr: 0x6000_9055, bytes: 1, pc: 0x4000_0100 }]);
    }

    #[test]
    fn misaligned_mmio_access_is_a_bus_error() {
        let mut b = bus();
        assert_eq!(b.read(0x6000_9055, Word, Load), Err(BusError::Misaligned { addr: 0x6000_9055, size: Word }));
        assert_eq!(b.write(0x6000_9055, Half, 0), Err(BusError::Misaligned { addr: 0x6000_9055, size: Half }));
    }

    #[test]
    fn strict_mode_turns_an_unmodeled_access_into_a_violation() {
        let mut b = bus_with(SocConfig { strict_mmio: true, ..SocConfig::default() });
        let _ = load(&mut b, unmodeled().addr);
        assert!(matches!(b.diag.take_violation(), Some(Diagnostic::UnmodeledMmio { .. })));
    }
}
```

Ajouter dans `periph/mod.rs` (module `tests`) :

```rust
    #[test]
    fn storage_starts_at_the_svd_reset_values_and_only_answers_its_own_registers() {
        let mut s = Storage::for_peripheral("IO_MUX", 0x6000_9000);
        let gpio20 = svd_regs::esp32c3::lookup(0x6000_9054).unwrap();
        assert_eq!(s.read(0x54), Some(gpio20.reset_value));
        assert!(s.write(0x54, 5));
        assert_eq!(s.read(0x54), Some(5));
        assert_eq!(s.read(0x60), None, "no IO_MUX register at +0x60");
        assert!(!s.write(0x60, 1));
        let mut picked = Storage::for_offsets(0x6000_8000, &[0x8C, 0x84, 0x84]);
        assert_eq!(picked.read(0x84), Some(svd_regs::esp32c3::lookup(0x6000_8084).unwrap().reset_value));
        assert_eq!(picked.read(0x88), None);
        assert!(picked.write(0x8C, 9));
    }
```

- [ ] **Step 3: Lancer, constater l'échec de compilation**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 4: Implémenter**

`crates/emu-core/src/peripheral.rs` (et `pub mod peripheral; pub use peripheral::Peripheral;` dans `lib.rs`) :

```rust
//! A memory-mapped device, seen as 32-bit registers.

/// The SoC bus splits sub-word accesses and resolves the device, so a peripheral only ever sees
/// whole registers at 4-byte-aligned offsets from its own base.
pub trait Peripheral: std::fmt::Debug {
    /// `None` means this model does not implement the register: the bus then answers with the
    /// documented reset value and reports the access.
    fn read(&mut self, offset: u32) -> Option<u32>;
    /// `false` means this model does not implement the register: the write is dropped and reported.
    fn write(&mut self, offset: u32, value: u32) -> bool;
}
```

`config.rs` : la structure `SocConfig` de la section Interfaces, avec `Default`. Commentaire de `STRAP_SPI_FAST_FLASH_BOOT` : valeur de `GPIO_STRAP_REG` affichée par la ROM en `boot:0x8 (SPI_FAST_FLASH_BOOT)` (rapport §D.3).

`diag.rs` :

```rust
//! What the emulator has to tell the user about the run: accesses it does not model, CPU exceptions.
use cpu_rv32::Exception;
use std::collections::BTreeMap;
use std::fmt;

/// Past this many distinct diagnostics, further ones are counted but no longer listed: a runaway
/// program must not grow the log without bound.
pub const MAX_DISTINCT_DIAGNOSTICS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Read,
    Write,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Diagnostic {
    /// Spec §5.2: `value` is what the read returned, or what the ignored write carried.
    UnmodeledMmio { addr: u32, direction: Direction, pc: u32, register: Option<(&'static str, &'static str)>, value: u32, reset_explicit: bool },
    /// An 8- or 16-bit store to a peripheral register (decision D9 of the SP1b-1 plan).
    SubWordWrite { addr: u32, bytes: u32, pc: u32 },
    Exception { cause: Exception, tval: u32, pc: u32 },
    /// Emitted once when [`MAX_DISTINCT_DIAGNOSTICS`] is reached.
    Suppressed,
}

/// Deduplication key: what the diagnostic is about, not where it was first seen.
type Key = (u8, u32, u32);

impl Diagnostic {
    fn key(&self) -> Key {
        match *self {
            Diagnostic::UnmodeledMmio { addr, direction: Direction::Read, .. } => (0, addr, 0),
            Diagnostic::UnmodeledMmio { addr, direction: Direction::Write, .. } => (1, addr, 0),
            Diagnostic::SubWordWrite { addr, .. } => (2, addr, 0),
            Diagnostic::Exception { cause, pc, .. } => (3, pc, cause.code()),
            Diagnostic::Suppressed => (4, 0, 0),
        }
    }

    const fn is_mmio(&self) -> bool {
        matches!(self, Diagnostic::UnmodeledMmio { .. } | Diagnostic::SubWordWrite { .. })
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Diagnostic::UnmodeledMmio { addr, direction, pc, register, value, reset_explicit } => {
                let what = match direction {
                    Direction::Read => "read",
                    Direction::Write => "write",
                };
                match register {
                    Some((peripheral, name)) => write!(f, "unmodeled register {what}: {peripheral}.{name} at {addr:#010x} (pc {pc:#010x}); ")?,
                    None => write!(f, "unmodeled {what} at {addr:#010x} (pc {pc:#010x}): no register is documented there; ")?,
                }
                match (direction, register.is_some() && reset_explicit) {
                    (Direction::Read, true) => write!(f, "returning its documented reset value {value:#010x}"),
                    (Direction::Read, false) => write!(f, "returning 0, the SVD documents no reset value"),
                    (Direction::Write, _) => write!(f, "value {value:#010x} ignored"),
                }
            }
            Diagnostic::SubWordWrite { addr, bytes, pc } => write!(
                f,
                "{bytes}-byte write to a 32-bit peripheral register at {addr:#010x} (pc {pc:#010x}); the whole register was written, its other bytes as zero"
            ),
            Diagnostic::Exception { cause, tval, pc } => write!(f, "CPU exception {cause:?} at pc {pc:#010x} (tval {tval:#010x})"),
            Diagnostic::Suppressed => write!(f, "more than {MAX_DISTINCT_DIAGNOSTICS} distinct diagnostics; further ones are counted but not listed"),
        }
    }
}

#[derive(Debug)]
pub struct Diagnostics {
    strict: bool,
    /// Occurrences per key. A `BTreeMap` keeps every observable order deterministic.
    seen: BTreeMap<Key, u64>,
    pending: Vec<Diagnostic>,
    violation: Option<Diagnostic>,
    capped: bool,
}

impl Diagnostics {
    #[must_use]
    pub fn new(strict: bool) -> Diagnostics {
        Diagnostics { strict, seen: BTreeMap::new(), pending: Vec::new(), violation: None, capped: false }
    }

    pub fn record(&mut self, diagnostic: Diagnostic) {
        if self.strict && self.violation.is_none() && diagnostic.is_mmio() {
            self.violation = Some(diagnostic.clone());
        }
        if let Some(count) = self.seen.get_mut(&diagnostic.key()) {
            *count += 1;
            return;
        }
        if self.seen.len() >= MAX_DISTINCT_DIAGNOSTICS {
            if !std::mem::replace(&mut self.capped, true) {
                self.pending.push(Diagnostic::Suppressed);
            }
            return;
        }
        self.seen.insert(diagnostic.key(), 1);
        self.pending.push(diagnostic);
    }

    /// Diagnostics seen for the first time since the last call, in order of first occurrence.
    pub fn drain(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.pending)
    }

    /// In strict mode, the first unmodeled access; `None` otherwise or once taken.
    pub fn take_violation(&mut self) -> Option<Diagnostic> {
        self.violation.take()
    }

    #[must_use]
    pub fn occurrences(&self, diagnostic: &Diagnostic) -> u64 {
        self.seen.get(&diagnostic.key()).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn distinct(&self) -> usize {
        self.seen.len()
    }
}
```

Le test `strict_mode_reports_the_first_mmio_diagnostic_as_a_violation` attend que la violation, une fois prise, ne se reproduise pas pour les diagnostics **déjà enregistrés** ; la machine s'arrête de toute façon à la première. Si le test et ce code divergent sur un détail, c'est le test qui fait foi.

`periph/mod.rs` :

```rust
//! The modelled peripherals of the ESP32-C3 and their place in the address space.
use crate::config::SocConfig;
use emu_core::Peripheral;

pub mod gpio;
pub mod io_mux;

pub use gpio::Gpio;
pub use io_mux::IoMux;

/// Registers deliberately modelled as plain read/write memory: configuration the emulator
/// acknowledges but does not act on. Listing them here is a decision; anything else is reported.
#[derive(Clone, Debug)]
pub struct Storage {
    /// `(offset, value)`, sorted by offset.
    regs: Vec<(u32, u32)>,
}

impl Storage {
    /// Every register the SVD documents for `peripheral`, at its reset value.
    #[must_use]
    pub fn for_peripheral(peripheral: &str, base: u32) -> Storage {
        let regs = svd_regs::esp32c3::registers()
            .iter()
            .filter(|r| r.peripheral == peripheral)
            .map(|r| (r.addr - base, r.reset_value))
            .collect();
        Storage { regs }
    }

    /// Only the registers at `offsets`, at their SVD reset value (0 when the SVD has none).
    #[must_use]
    pub fn for_offsets(base: u32, offsets: &[u32]) -> Storage {
        let mut regs: Vec<(u32, u32)> = offsets
            .iter()
            .map(|&offset| (offset, svd_regs::esp32c3::lookup(base + offset).map_or(0, |r| r.reset_value)))
            .collect();
        regs.sort_unstable();
        regs.dedup();
        Storage { regs }
    }

    fn index(&self, offset: u32) -> Option<usize> {
        self.regs.binary_search_by_key(&offset, |&(o, _)| o).ok()
    }

    #[must_use]
    pub fn read(&self, offset: u32) -> Option<u32> {
        self.index(offset).map(|i| self.regs[i].1)
    }

    pub fn write(&mut self, offset: u32, value: u32) -> bool {
        match self.index(offset) {
            Some(i) => {
                self.regs[i].1 = value;
                true
            }
            None => false,
        }
    }
}

/// Address ranges of the modelled blocks (`reg_base.h`; report §A.2). Ends are exclusive.
const GPIO: (u32, u32) = (0x6000_4000, 0x6000_4F00);
const IO_MUX: (u32, u32) = (0x6000_9000, 0x6000_A000);

#[derive(Debug)]
pub struct Peripherals {
    pub gpio: Gpio,
    pub io_mux: IoMux,
}

impl Peripherals {
    #[must_use]
    pub fn new(config: &SocConfig) -> Peripherals {
        Peripherals { gpio: Gpio::new(config.strapping), io_mux: IoMux::new() }
    }

    /// The device that owns the register at `addr`, with its base address.
    pub(crate) fn device(&mut self, addr: u32) -> Option<(&mut dyn Peripheral, u32)> {
        let within = |block: (u32, u32)| (block.0..block.1).contains(&addr);
        if within(GPIO) {
            Some((&mut self.gpio, GPIO.0))
        } else if within(IO_MUX) {
            Some((&mut self.io_mux, IO_MUX.0))
        } else {
            None
        }
    }
}
```

`periph/gpio.rs` :

```rust
//! GPIO block. Only the strapping register is modelled so far; pins arrive with the board wiring (SP2).
use emu_core::Peripheral;

/// `GPIO_STRAP_REG`: the strapping pins sampled at reset. Bit 3 set selects SPI flash boot (report §D.3).
const STRAP: u32 = 0x38;

#[derive(Clone, Debug)]
pub struct Gpio {
    strapping: u32,
}

impl Gpio {
    #[must_use]
    pub const fn new(strapping: u32) -> Gpio {
        Gpio { strapping }
    }
}

impl Peripheral for Gpio {
    fn read(&mut self, offset: u32) -> Option<u32> {
        (offset == STRAP).then_some(self.strapping)
    }
    fn write(&mut self, _offset: u32, _value: u32) -> bool {
        false
    }
}
```

`periph/io_mux.rs` : `pub struct IoMux { pads: Storage }`, `IoMux::new()` = `Storage::for_peripheral("IO_MUX", 0x6000_9000)` (la base vient d'une constante partagée avec `mod.rs`), `Peripheral` délègue à `pads`. Commentaire : la configuration des broches est mémorisée ; elle n'a d'effet qu'avec le câblage des cartes (SP2).

`bus.rs` :

```rust
//! The ESP32-C3 address space. Memory map: `docs/research/esp32c3-boot-chain.md`, §A.1.
use crate::config::SocConfig;
use crate::diag::{Diagnostic, Diagnostics, Direction};
use crate::memmap::{
    DROM_BASE, DROM_ROM_OFFSET, DROM_SIZE, PERIPH_BASE, PERIPH_SIZE, ROM_BASE, ROM_SIZE, RTC_RAM_BASE, RTC_RAM_SIZE,
    SRAM_DBUS_BASE, SRAM_IBUS_BASE, SRAM_SIZE, SRAM0_BASE, SRAM0_SIZE,
};
use crate::periph::Peripherals;
use crate::rom::RomImage;
use emu_core::{AccessKind, AccessSize, Bus, BusError, Ram, ReadOutcome};

const REGISTER_BYTES: u32 = 4;

#[derive(Debug)]
pub struct SocBus {
    rom: RomImage,
    /// The 384 KiB SRAM, addressed here by its data-bus address.
    sram: Ram,
    sram0: Ram,
    rtc: Ram,
    pub periph: Peripherals,
    pub diag: Diagnostics,
    /// PC of the instruction being executed, for diagnostics.
    pc: u32,
}

#[derive(Clone, Copy)]
enum Region {
    /// Offset in the ROM image.
    Rom(u32),
    /// Data-bus address of the SRAM cell, whichever bus the access came through.
    Sram(u32),
    Sram0,
    Rtc,
    Periph,
}

/// Offset of `[addr, addr + len)` in the block at `base`, when it lies entirely inside it.
fn offset_in(addr: u32, len: u32, base: u32, size: u32) -> Option<u32> {
    let offset = addr.checked_sub(base)?;
    (offset.checked_add(len)? <= size).then_some(offset)
}

fn locate(addr: u32, len: u32) -> Option<Region> {
    let at = |base, size| offset_in(addr, len, base, size);
    if let Some(offset) = at(ROM_BASE, ROM_SIZE) {
        Some(Region::Rom(offset))
    } else if let Some(offset) = at(DROM_BASE, DROM_SIZE) {
        Some(Region::Rom(offset + DROM_ROM_OFFSET))
    } else if let Some(offset) = at(SRAM_DBUS_BASE, SRAM_SIZE).or_else(|| at(SRAM_IBUS_BASE, SRAM_SIZE)) {
        Some(Region::Sram(SRAM_DBUS_BASE + offset))
    } else if at(SRAM0_BASE, SRAM0_SIZE).is_some() {
        Some(Region::Sram0)
    } else if at(RTC_RAM_BASE, RTC_RAM_SIZE).is_some() {
        Some(Region::Rtc)
    } else if at(PERIPH_BASE, PERIPH_SIZE).is_some() {
        Some(Region::Periph)
    } else {
        None
    }
}

const fn size_mask(size: AccessSize) -> u32 {
    match size {
        AccessSize::Byte => 0xff,
        AccessSize::Half => 0xffff,
        AccessSize::Word => u32::MAX,
    }
}

impl SocBus {
    #[must_use]
    pub fn new(rom: RomImage, config: &SocConfig) -> SocBus {
        SocBus {
            rom,
            sram: Ram::new(SRAM_DBUS_BASE, SRAM_SIZE as usize),
            sram0: Ram::new(SRAM0_BASE, SRAM0_SIZE as usize),
            rtc: Ram::new(RTC_RAM_BASE, RTC_RAM_SIZE as usize),
            periph: Peripherals::new(config),
            diag: Diagnostics::new(config.strict_mmio),
            pc: 0,
        }
    }

    /// Tells the bus which instruction is running, so diagnostics can name it.
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }

    fn rom_read(&self, offset: u32, size: AccessSize) -> u32 {
        // INVARIANT: `locate` checked that the whole access lies inside the ROM.
        let start = offset as usize;
        self.rom.bytes()[start..start + size.bytes() as usize].iter().rev().fold(0, |acc, &b| (acc << 8) | u32::from(b))
    }

    /// Spec §5.2: an unmodelled register reads as its SVD reset value; the access is reported once.
    fn unmodeled(&mut self, reg_addr: u32, direction: Direction, written: u32) -> u32 {
        let info = svd_regs::esp32c3::lookup(reg_addr);
        let reset = info.map_or(0, |r| r.reset_value);
        self.diag.record(Diagnostic::UnmodeledMmio {
            addr: reg_addr,
            direction,
            pc: self.pc,
            register: info.map(|r| (r.peripheral, r.name)),
            value: if direction == Direction::Read { reset } else { written },
            reset_explicit: info.is_some_and(|r| r.reset_explicit),
        });
        reset
    }

    fn mmio_read(&mut self, addr: u32, size: AccessSize) -> Result<ReadOutcome, BusError> {
        if addr % size.bytes() != 0 {
            return Err(BusError::Misaligned { addr, size });
        }
        let reg_addr = addr & !(REGISTER_BYTES - 1);
        let modelled = self.periph.device(reg_addr).and_then(|(device, base)| device.read(reg_addr - base));
        let word = match modelled {
            Some(word) => word,
            None => self.unmodeled(reg_addr, Direction::Read, 0),
        };
        let shift = 8 * (addr - reg_addr);
        Ok(ReadOutcome { value: (word >> shift) & size_mask(size), stall: 0 })
    }

    fn mmio_write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        if addr % size.bytes() != 0 {
            return Err(BusError::Misaligned { addr, size });
        }
        let reg_addr = addr & !(REGISTER_BYTES - 1);
        let word = (value & size_mask(size)) << (8 * (addr - reg_addr));
        if size != AccessSize::Word {
            self.diag.record(Diagnostic::SubWordWrite { addr, bytes: size.bytes(), pc: self.pc });
        }
        let handled = self.periph.device(reg_addr).is_some_and(|(device, base)| device.write(reg_addr - base, word));
        if !handled {
            self.unmodeled(reg_addr, Direction::Write, word);
        }
        Ok(0)
    }
}

impl Bus for SocBus {
    fn read(&mut self, addr: u32, size: AccessSize, kind: AccessKind) -> Result<ReadOutcome, BusError> {
        match locate(addr, size.bytes()) {
            Some(Region::Rom(offset)) => Ok(ReadOutcome { value: self.rom_read(offset, size), stall: 0 }),
            Some(Region::Sram(dbus_addr)) => self.sram.read(dbus_addr, size, kind),
            Some(Region::Sram0) => self.sram0.read(addr, size, kind),
            Some(Region::Rtc) => self.rtc.read(addr, size, kind),
            Some(Region::Periph) if kind != AccessKind::Fetch => self.mmio_read(addr, size),
            Some(Region::Periph) | None => Err(BusError::Unmapped { addr, kind }),
        }
    }

    fn write(&mut self, addr: u32, size: AccessSize, value: u32) -> Result<u32, BusError> {
        match locate(addr, size.bytes()) {
            Some(Region::Sram(dbus_addr)) => self.sram.write(dbus_addr, size, value),
            Some(Region::Sram0) => self.sram0.write(addr, size, value),
            Some(Region::Rtc) => self.rtc.write(addr, size, value),
            Some(Region::Periph) => self.mmio_write(addr, size, value),
            // Decision D10: a store to the mask ROM raises a store access fault.
            Some(Region::Rom(_)) | None => Err(BusError::Unmapped { addr, kind: AccessKind::Store }),
        }
    }

    fn page_version(&self, addr: u32) -> u64 {
        // The SRAM keeps one version per physical page: code decoded at an instruction-bus address is
        // invalidated by a store through the data bus, because both resolve to the same cell.
        match locate(addr, 1) {
            Some(Region::Sram(dbus_addr)) => self.sram.page_version(dbus_addr),
            Some(Region::Sram0) => self.sram0.page_version(addr),
            Some(Region::Rtc) => self.rtc.page_version(addr),
            Some(Region::Rom(_) | Region::Periph) | None => 0,
        }
    }
}
```

Une erreur renvoyée par la `Ram` de la SRAM porte l'adresse du bus de données ; `locate` ayant déjà validé l'accès, ce cas ne se produit pas. Si un `Ram::read` désaligné renvoie une erreur que les tests n'attendent pas, c'est le test qui fait foi.

- [ ] **Step 5: Lancer les tests**

Run: `cargo test -p soc-esp32c3 -p emu-core > /tmp/soc.log 2>&1; echo "exit=$?"; cargo clippy -p soc-esp32c3 -p emu-core --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"`
Expected: `exit=0`, `clippy=0`.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/emu-core crates/soc-esp32c3
/usr/bin/git commit -m "feat(soc-esp32c3): add the SoC bus with memory aliases, MMIO dispatch and named diagnostics" -- crates/emu-core crates/soc-esp32c3
```

---

### Task 6: Périphériques système que la ROM touche avant sa bannière

**Files:**
- Create: `crates/soc-esp32c3/src/periph/intmatrix.rs`, `rtc_cntl.rs`, `system.rs`, `efuse.rs`, `assist_debug.rs` (même dossier)
- Modify: `crates/soc-esp32c3/src/periph/mod.rs`

**Interfaces:**
- Consumes: `emu_core::Peripheral`, `Storage`, `SocConfig`, `ChipRevision` (Tasks 4 et 5).
- Produces: `Peripherals` gagne les champs `pub intmatrix: IntMatrix`, `pub rtc_cntl: RtcCntl`, `pub system: System`, `pub efuse: Efuse`, `pub assist_debug: AssistDebug`, tous `impl Peripheral`. Constructeurs : `IntMatrix::new()`, `RtcCntl::new()`, `System::new()`, `Efuse::new(ChipRevision)`, `AssistDebug::new()`.

Plages d'adresses (fins exclues) à ajouter dans `periph/mod.rs` : `RTC_CNTL (0x6000_8000, 0x6000_8800)`, `EFUSE (0x6000_8800, 0x6000_9000)`, `SYSTEM (0x600C_0000, 0x600C_1000)`, `INTMATRIX (0x600C_2000, 0x600C_3000)`, `ASSIST_DEBUG (0x600C_E000, 0x600C_F000)`.

Chaque registre ci-dessous est celui que la ROM lit ou écrit avant sa bannière : `docs/research/esp32c3-boot-chain.md`, §J, lignes 0 à 12, et §B.3, §E.1, §E.2, §H pour les formats.

- [ ] **Step 1: Écrire les tests (un module `tests` par fichier)**

`intmatrix.rs` :

```rust
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
        assert_eq!((m.read(0x54), m.read(0x128), m.read(0x194), m.read(0x104)), (Some(5), Some(1), Some(1), Some(1 << 5)));
    }

    #[test]
    fn fields_keep_only_their_defined_bits() {
        let mut m = IntMatrix::new();
        m.write(0xF4, 0xFFFF_FFFF); // last source map
        m.write(0x114 + 4 * 31, 0xFFFF_FFFF); // CPU_INT_PRI_31
        m.write(0x194, 0xFFFF_FFFF);
        assert_eq!((m.read(0xF4), m.read(0x114 + 4 * 31), m.read(0x194)), (Some(0x1F), Some(0xF), Some(0xF)));
    }

    #[test]
    fn status_registers_read_zero_while_no_source_exists_and_the_rest_is_unmodeled() {
        let mut m = IntMatrix::new();
        assert_eq!((m.read(0xF8), m.read(0xFC), m.read(0x110)), (Some(0), Some(0), Some(0)));
        assert!(m.write(0x10C, 0xFFFF_FFFF), "CPU_INT_CLEAR accepts a write");
        assert_eq!(m.read(0x7FC), None, "INTERRUPT_DATE is not modelled");
        assert!(!m.write(0xF8, 1), "status registers are read-only");
    }
}
```

`rtc_cntl.rs` :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn power_on_reset_cause_is_reported() {
        let mut rtc = RtcCntl::new();
        let state = rtc.read(0x38).unwrap();
        assert_eq!(state & 0x3F, 1, "RESET_CAUSE_PROCPU = POWERON");
        assert_eq!(state & (1 << 20), 0, "no JTAG reset flag");
        assert_eq!(state & !0x3F, 0x3000, "the other bits keep their documented reset value");
    }

    #[test]
    fn store_registers_keep_what_the_rom_writes() {
        let mut rtc = RtcCntl::new();
        for (i, offset) in [0x50, 0x54, 0x58, 0x5C, 0xB8, 0xBC, 0xC0, 0xC4].into_iter().enumerate() {
            let value = 0x1000 + u32::try_from(i).unwrap();
            assert_eq!(rtc.read(offset), Some(0));
            assert!(rtc.write(offset, value));
            assert_eq!(rtc.read(offset), Some(value));
        }
    }

    #[test]
    fn pad_and_power_configuration_is_plain_storage_from_its_reset_value() {
        let mut rtc = RtcCntl::new();
        for offset in [0x84, 0x8C, 0xAC, 0xD0, 0xD4] {
            let reset = svd_regs::esp32c3::lookup(0x6000_8000 + offset).unwrap().reset_value;
            assert_eq!(rtc.read(offset), Some(reset), "{offset:#x}");
            assert!(rtc.write(offset, 0x55AA));
            assert_eq!(rtc.read(offset), Some(0x55AA));
        }
        assert_eq!(rtc.read(0xAC).map(|v| v & 1), Some(0), "super watchdog reset flag clear");
    }

    #[test]
    fn everything_else_is_left_to_the_unmodeled_path() {
        let mut rtc = RtcCntl::new();
        assert_eq!(rtc.read(0x00), None, "OPTIONS0: software reset arrives in SP1b-3");
        assert!(!rtc.write(0x38, 0), "RESET_STATE is not writable here");
        assert!(!rtc.write(0x90, 0), "watchdog configuration arrives in SP1b-3");
    }
}
```

`system.rs` :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn sysclk_conf_reports_a_40_mhz_crystal_selected_undivided() {
        let mut sys = System::new();
        let conf = sys.read(0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40, "CLK_XTAL_FREQ");
        assert_eq!((conf >> 10) & 3, 0, "SOC_CLK_SEL = XTAL");
        assert_eq!(conf & 0x3FF, 1, "PRE_DIV_CNT");
    }

    #[test]
    fn crystal_frequency_is_read_only() {
        let mut sys = System::new();
        assert!(sys.write(0x58, 0xFFFF_FFFF));
        let conf = sys.read(0x58).unwrap();
        assert_eq!((conf >> 12) & 0x7F, 40);
        assert_eq!(conf & 0xFFF, 0xFFF, "divider and clock select are writable");
        assert_eq!(conf >> 19, 0, "CLK_DIV_EN and above stay clear");
    }

    #[test]
    fn clock_gates_and_resets_are_storage_from_their_reset_values() {
        let mut sys = System::new();
        for offset in [0x00, 0x04, 0x10, 0x14, 0x18, 0x1C] {
            let reset = svd_regs::esp32c3::lookup(0x600C_0000 + offset).unwrap().reset_value;
            assert_eq!(sys.read(offset), Some(reset), "{offset:#x}");
            assert!(sys.write(offset, reset | 4));
            assert_eq!(sys.read(offset), Some(reset | 4));
        }
        assert_eq!(sys.read(0x08), None, "CPU_PER_CONF belongs to the clock tree (SP1b-2)");
    }
}
```

`efuse.rs` :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    fn revision(efuse: &mut Efuse) -> (u32, u32) {
        let sys3 = efuse.read(0x50).unwrap();
        let sys5 = efuse.read(0x58).unwrap();
        let minor = (((sys5 >> 23) & 1) << 3) | ((sys3 >> 18) & 7);
        ((sys5 >> 24) & 3, minor)
    }

    #[test]
    fn blank_chip_reads_zero_everywhere_except_the_wafer_version() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(revision(&mut efuse), (0, 3));
        for offset in (0x2C..=0x1C4).step_by(4).filter(|o| ![0x50, 0x58].contains(o)) {
            assert_eq!(efuse.read(offset), Some(0), "{offset:#x}");
        }
        assert_eq!(efuse.read(0x50), Some(3 << 18));
    }

    #[test]
    fn minor_revision_is_split_across_two_words() {
        let mut efuse = Efuse::new(ChipRevision::new(1, 9).unwrap());
        assert_eq!(revision(&mut efuse), (1, 9));
        assert_eq!(efuse.read(0x58), Some((1 << 24) | (1 << 23)));
    }

    #[test]
    fn programming_registers_and_writes_are_not_modelled() {
        let mut efuse = Efuse::new(ChipRevision::V0_3);
        assert_eq!(efuse.read(0x00), None, "PGM_DATA0");
        assert_eq!(efuse.read(0x1C8), None, "CLK");
        assert_eq!(efuse.read(0x1D4), None, "CMD");
        assert!(!efuse.write(0x30, 1), "the read mirror is read-only");
    }
}
```

`assist_debug.rs` :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    #[test]
    fn no_pc_was_recorded_before_a_power_on_reset() {
        let mut dbg = AssistDebug::new();
        assert_eq!(dbg.read(0x48), Some(0), "CORE_0_RCD_PDEBUGPC: the ROM prints `Saved PC` only when non-zero");
        assert_eq!(dbg.read(0x4C), Some(0), "CORE_0_RCD_PDEBUGSP");
    }

    #[test]
    fn record_enable_is_remembered_and_the_rest_is_unmodeled() {
        let mut dbg = AssistDebug::new();
        assert!(dbg.write(0x44, 3));
        assert_eq!(dbg.read(0x44), Some(3));
        assert_eq!(dbg.read(0x00), None);
        assert!(!dbg.write(0x48, 1), "the recorded PC is read-only");
    }
}
```

Ajouter dans `bus.rs` (module `tests`) :

```rust
    #[test]
    fn rtc_cntl_and_efuse_share_a_page_but_not_their_registers() {
        let mut b = bus();
        assert_eq!(load(&mut b, 0x6000_8038) & 0x3F, 1, "RTC_CNTL.RESET_STATE");
        assert_eq!(load(&mut b, 0x6000_8850), 3 << 18, "EFUSE.RD_MAC_SPI_SYS_3 of a v0.3 chip");
        assert_eq!(load(&mut b, 0x600C_0058) >> 12 & 0x7F, 40, "SYSTEM.SYSCLK_CONF");
        assert_eq!(load(&mut b, 0x600C_2100), 1, "INTERRUPT_CORE0.CLOCK_GATE");
        assert_eq!(load(&mut b, 0x600C_E048), 0, "ASSIST_DEBUG.CORE_0_RCD_PDEBUGPC");
        assert!(b.diag.drain().is_empty());
    }
```

- [ ] **Step 2: Lancer, constater l'échec de compilation**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implémenter les cinq périphériques**

`intmatrix.rs` :

```rust
//! `INTERRUPT_CORE0`: routes the 62 peripheral interrupt sources to the 31 CPU interrupt lines.
//! Register layout: `interrupt_core0_reg.h` (report §B.3).
//! This plan keeps the configuration the software writes; no source is modelled yet, so nothing is
//! ever pending. Delivery to the CPU arrives in SP1b-3.
use emu_core::Peripheral;

pub const SOURCE_COUNT: usize = 62;
pub const CPU_LINE_COUNT: usize = 32;

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
        IntMatrix { map: [0; SOURCE_COUNT], clock_gate: CLOCK_GATE_RESET, enable: 0, edge_type: 0, priority: [0; CPU_LINE_COUNT], threshold: 0 }
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
            CLOCK_GATE => self.clock_gate = value & 1,
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
```

`rtc_cntl.rs` :

```rust
//! `RTC_CNTL`: reset cause, retention registers, pad and power configuration.
//! Source: report §E.2. Software reset, the RTC timer and the watchdogs arrive in SP1b-3.
use crate::periph::Storage;
use emu_core::Peripheral;

pub(crate) const BASE: u32 = 0x6000_8000;
const RESET_STATE: u32 = 0x38;
/// `RTC_CNTL_STORE0..7`: they survive a reset; the ROM keeps the APB frequency in STORE5.
const STORE: [u32; 8] = [0x50, 0x54, 0x58, 0x5C, 0xB8, 0xBC, 0xC0, 0xC4];
/// Configuration the ROM reads back and modifies before its banner: PWC, DIG_ISO, SWD_CONF,
/// PAD_HOLD, DIG_PAD_HOLD. Nothing acts on it yet.
const CONFIG: [u32; 5] = [0x84, 0x8C, 0xAC, 0xD0, 0xD4];
/// `RESET_CAUSE_PROCPU`, bits [5:0] of `RESET_STATE` (`soc/reset_reasons.h`).
const RESET_CAUSE_MASK: u32 = 0x3F;
const RESET_CAUSE_POWERON: u32 = 0x01;

#[derive(Clone, Debug)]
pub struct RtcCntl {
    reset_cause: u32,
    /// The bits of `RESET_STATE` other than the cause, at their documented reset value.
    reset_state_other: u32,
    store: [u32; 8],
    config: Storage,
}

impl RtcCntl {
    #[must_use]
    pub fn new() -> RtcCntl {
        let documented = svd_regs::esp32c3::lookup(BASE + RESET_STATE).map_or(0, |r| r.reset_value);
        RtcCntl {
            reset_cause: RESET_CAUSE_POWERON,
            reset_state_other: documented & !RESET_CAUSE_MASK,
            store: [0; 8],
            config: Storage::for_offsets(BASE, &CONFIG),
        }
    }
}

impl Default for RtcCntl {
    fn default() -> Self {
        RtcCntl::new()
    }
}

impl Peripheral for RtcCntl {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == RESET_STATE {
            return Some(self.reset_state_other | self.reset_cause);
        }
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            return Some(self.store[i]);
        }
        self.config.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if let Some(i) = STORE.iter().position(|&o| o == offset) {
            self.store[i] = value;
            return true;
        }
        self.config.write(offset, value)
    }
}
```

`system.rs` :

```rust
//! `SYSTEM`: clock selection, peripheral clock gates and resets. Source: report §E.1.
//! The clock tree itself (PLL, CPU frequency switch) arrives in SP1b-2.
use crate::periph::Storage;
use emu_core::Peripheral;

pub(crate) const BASE: u32 = 0x600C_0000;
const SYSCLK_CONF: u32 = 0x58;
/// CPU_PERI_CLK_EN, CPU_PERI_RST_EN, PERIP_CLK_EN0/1, PERIP_RST_EN0/1: gates and reset pulses the
/// ROM toggles around each peripheral it sets up. Nothing acts on them yet.
const GATES: [u32; 6] = [0x00, 0x04, 0x10, 0x14, 0x18, 0x1C];
/// `SYSCLK_CONF`: bits [9:0] PRE_DIV_CNT and [11:10] SOC_CLK_SEL are writable.
const SYSCLK_WRITABLE: u32 = 0xFFF;
/// Reset: PRE_DIV_CNT = 1, SOC_CLK_SEL = 0 (the crystal drives the CPU).
const SYSCLK_RESET: u32 = 1;
/// `CLK_XTAL_FREQ`, bits [18:12], read-only: the ROM derives its UART divider from it
/// (`ROM3:ets_get_apb_freq@0x40049866`). The Xteink boards carry a 40 MHz crystal.
const XTAL_FREQ_SHIFT: u32 = 12;
const XTAL_FREQ_MHZ: u32 = 40;

#[derive(Clone, Debug)]
pub struct System {
    sysclk_writable: u32,
    gates: Storage,
}

impl System {
    #[must_use]
    pub fn new() -> System {
        System { sysclk_writable: SYSCLK_RESET, gates: Storage::for_offsets(BASE, &GATES) }
    }
}

impl Default for System {
    fn default() -> Self {
        System::new()
    }
}

impl Peripheral for System {
    fn read(&mut self, offset: u32) -> Option<u32> {
        if offset == SYSCLK_CONF {
            return Some(self.sysclk_writable | (XTAL_FREQ_MHZ << XTAL_FREQ_SHIFT));
        }
        self.gates.read(offset)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if offset == SYSCLK_CONF {
            self.sysclk_writable = value & SYSCLK_WRITABLE;
            return true;
        }
        self.gates.write(offset, value)
    }
}
```

`efuse.rs` :

```rust
//! eFuse read mirror of a blank chip. Layout: `efuse_reg.h` (report §H).
//! Programming, the MAC address and the block version arrive when the bootloader needs them (SP1b-2).
use crate::chip::ChipRevision;
use emu_core::Peripheral;

/// First and last register of the read mirror: `RD_WR_DIS` to `RD_RS_ERR1`.
const RD_FIRST: u32 = 0x2C;
const RD_LAST: u32 = 0x1C4;
const RD_WORDS: usize = ((RD_LAST - RD_FIRST) / 4 + 1) as usize;
/// `RD_MAC_SPI_SYS_3` bits [20:18]: `WAFER_VERSION_MINOR_LO`.
const RD_MAC_SPI_SYS_3: u32 = 0x50;
const WAFER_MINOR_LO_SHIFT: u32 = 18;
const WAFER_MINOR_LO_MASK: u32 = 0x7;
/// `RD_MAC_SPI_SYS_5` bit 23: `WAFER_VERSION_MINOR_HI`; bits [25:24]: `WAFER_VERSION_MAJOR`.
const RD_MAC_SPI_SYS_5: u32 = 0x58;
const WAFER_MINOR_HI_SHIFT: u32 = 23;
const WAFER_MAJOR_SHIFT: u32 = 24;

#[derive(Clone, Debug)]
pub struct Efuse {
    read_mirror: [u32; RD_WORDS],
}

const fn word_index(offset: u32) -> usize {
    ((offset - RD_FIRST) / 4) as usize
}

impl Efuse {
    /// A chip with every fuse blank except its wafer version: no secure boot, no flash encryption,
    /// both ROM consoles enabled, default SPI pads.
    #[must_use]
    pub fn new(revision: ChipRevision) -> Efuse {
        let (major, minor) = (u32::from(revision.major()), u32::from(revision.minor()));
        let mut read_mirror = [0; RD_WORDS];
        read_mirror[word_index(RD_MAC_SPI_SYS_3)] = (minor & WAFER_MINOR_LO_MASK) << WAFER_MINOR_LO_SHIFT;
        read_mirror[word_index(RD_MAC_SPI_SYS_5)] = ((minor >> 3) << WAFER_MINOR_HI_SHIFT) | (major << WAFER_MAJOR_SHIFT);
        Efuse { read_mirror }
    }
}

impl Peripheral for Efuse {
    fn read(&mut self, offset: u32) -> Option<u32> {
        (RD_FIRST..=RD_LAST).contains(&offset).then(|| self.read_mirror[word_index(offset)])
    }

    fn write(&mut self, _offset: u32, _value: u32) -> bool {
        false
    }
}
```

`assist_debug.rs` :

```rust
//! `ASSIST_DEBUG`: the PC recorder the ROM consults to print `Saved PC` after a watchdog reset.
//! Source: report §J, line 10. Recording a PC at reset arrives with the watchdogs (SP1b-3).
use emu_core::Peripheral;

const CORE_0_RCD_EN: u32 = 0x44;
const CORE_0_RCD_PDEBUGPC: u32 = 0x48;
const CORE_0_RCD_PDEBUGSP: u32 = 0x4C;

#[derive(Clone, Debug, Default)]
pub struct AssistDebug {
    record_enable: u32,
}

impl AssistDebug {
    #[must_use]
    pub const fn new() -> AssistDebug {
        AssistDebug { record_enable: 0 }
    }
}

impl Peripheral for AssistDebug {
    fn read(&mut self, offset: u32) -> Option<u32> {
        match offset {
            CORE_0_RCD_EN => Some(self.record_enable),
            // Nothing was recorded: this is a power-on reset.
            CORE_0_RCD_PDEBUGPC | CORE_0_RCD_PDEBUGSP => Some(0),
            _ => None,
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        if offset == CORE_0_RCD_EN {
            self.record_enable = value;
        }
        offset == CORE_0_RCD_EN
    }
}
```

Dans `periph/mod.rs` : déclarer les cinq modules, ajouter les champs et les plages, construire `Efuse::new(config.chip_revision)`, et étendre `device` (une branche par plage, même forme que pour `GPIO`). Vérifier dans le SVD (`svd_regs::esp32c3::lookup`) que les noms des registres aux décalages utilisés correspondent bien aux noms cités en commentaire ; signaler tout écart au lieu d'adapter le commentaire en silence.

- [ ] **Step 4: Lancer les tests**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"; cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"`
Expected: `exit=0`, `clippy=0`.

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add crates/soc-esp32c3
/usr/bin/git commit -m "feat(soc-esp32c3): model the system peripherals the ROM uses before its banner" -- crates/soc-esp32c3
```

---

### Task 7: UART et USB-Serial-JTAG

**Files:**
- Create: `crates/soc-esp32c3/src/periph/uart.rs`, `crates/soc-esp32c3/src/periph/usb_serial_jtag.rs`
- Modify: `crates/soc-esp32c3/src/periph/mod.rs`

**Interfaces:**
- Consumes: `emu_core::Peripheral`, `Storage`.
- Produces:
  ```rust
  pub struct Uart;           impl Uart { pub fn new(base: u32) -> Uart; pub fn take_tx(&mut self) -> Vec<u8> }
  pub struct UsbSerialJtag;  impl UsbSerialJtag { pub fn new() -> UsbSerialJtag; pub fn take_tx(&mut self) -> Vec<u8> }
  // Peripherals gagne : pub uart0: Uart, pub uart1: Uart, pub usb_serial_jtag: UsbSerialJtag
  ```
  Plages : `UART0 (0x6000_0000, 0x6000_1000)`, `UART1 (0x6001_0000, 0x6001_1000)`, `USB_SERIAL_JTAG (0x6004_3000, 0x6004_4000)`.

Source : `docs/research/esp32c3-boot-chain.md`, §G.2 et §G.3. La ROM émet un octet en écrivant `FIFO` après avoir attendu que `TXFIFO_CNT` (bits 25:16 de `STATUS`) passe sous 128, et attend qu'il retombe à 0 pour vider. Côté USB, elle n'écrit `EP1` que si le bit 1 de `EP1_CONF` vaut 1, et valide le paquet en écrivant 1 dans `EP1_CONF` à chaque fin de ligne.

- [ ] **Step 1: Écrire les tests**

`uart.rs` :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    const UART0: u32 = 0x6000_0000;

    #[test]
    fn bytes_written_to_the_fifo_come_out_in_order_and_only_once() {
        let mut uart = Uart::new(UART0);
        for byte in *b"ESP" {
            assert!(uart.write(0x00, u32::from(byte)));
        }
        assert_eq!(uart.take_tx(), b"ESP");
        assert!(uart.take_tx().is_empty());
    }

    #[test]
    fn only_the_low_byte_of_a_fifo_write_is_transmitted() {
        let mut uart = Uart::new(UART0);
        uart.write(0x00, 0xFFFF_FF41);
        assert_eq!(uart.take_tx(), b"A");
    }

    #[test]
    fn transmit_fifo_always_reads_empty_so_the_rom_never_waits() {
        let mut uart = Uart::new(UART0);
        for _ in 0..200 {
            uart.write(0x00, 0x55);
        }
        let status = uart.read(0x1C).unwrap();
        assert_eq!((status >> 16) & 0x3FF, 0, "TXFIFO_CNT");
        assert_eq!(status & 0x3FF, 0, "RXFIFO_CNT");
    }

    #[test]
    fn line_configuration_is_storage_from_its_reset_value() {
        let mut uart = Uart::new(UART0);
        for offset in [0x0C, 0x14, 0x20, 0x24, 0x78] {
            let reset = svd_regs::esp32c3::lookup(UART0 + offset).unwrap().reset_value;
            assert_eq!(uart.read(offset), Some(reset), "{offset:#x}");
            assert!(uart.write(offset, 0x15B));
            assert_eq!(uart.read(offset), Some(0x15B));
        }
    }

    #[test]
    fn interrupt_clear_is_accepted_and_interrupt_status_is_not_modelled_yet() {
        let mut uart = Uart::new(UART0);
        assert!(uart.write(0x10, 0xFFFF_FFFF));
        assert_eq!(uart.read(0x04), None, "INT_RAW arrives with interrupts (SP1b-3)");
        assert_eq!(uart.read(0x08), None, "INT_ST arrives with interrupts (SP1b-3)");
    }

    #[test]
    fn second_uart_uses_its_own_reset_values() {
        let mut uart1 = Uart::new(0x6001_0000);
        let reset = svd_regs::esp32c3::lookup(0x6001_0014).unwrap().reset_value;
        assert_eq!(uart1.read(0x14), Some(reset));
    }
}
```

`usb_serial_jtag.rs` :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Peripheral;

    fn send(usb: &mut UsbSerialJtag, text: &[u8]) {
        for &byte in text {
            assert!(usb.write(0x00, u32::from(byte)));
        }
    }

    #[test]
    fn endpoint_always_has_room_and_no_received_data() {
        let mut usb = UsbSerialJtag::new();
        let conf = usb.read(0x04).unwrap();
        assert_eq!(conf & 0b010, 0b010, "SERIAL_IN_EP_DATA_FREE: the ROM drops the byte otherwise");
        assert_eq!(conf & 0b101, 0, "WR_DONE reads back clear, no data from the host");
    }

    #[test]
    fn bytes_reach_the_host_when_the_packet_is_committed() {
        let mut usb = UsbSerialJtag::new();
        send(&mut usb, b"boot\n");
        assert!(usb.take_tx().is_empty(), "nothing is sent before WR_DONE");
        assert!(usb.write(0x04, 1));
        assert_eq!(usb.take_tx(), b"boot\n");
        assert!(usb.take_tx().is_empty());
    }

    #[test]
    fn full_packet_is_sent_without_waiting_for_a_commit() {
        let mut usb = UsbSerialJtag::new();
        send(&mut usb, &[b'x'; 64]);
        assert_eq!(usb.take_tx().len(), 64);
        send(&mut usb, b"y");
        assert!(usb.take_tx().is_empty());
    }

    #[test]
    fn committing_an_empty_packet_sends_nothing_and_other_registers_are_unmodeled() {
        let mut usb = UsbSerialJtag::new();
        assert!(usb.write(0x04, 1));
        assert!(usb.take_tx().is_empty());
        assert_eq!(usb.read(0x08), None, "INT_RAW arrives with interrupts (SP1b-3)");
        assert!(!usb.write(0x18, 0), "CONF0");
        assert_eq!(usb.read(0x00), None, "no host-to-device data yet");
    }
}
```

Ajouter dans `bus.rs` (module `tests`) :

```rust
    #[test]
    fn consoles_are_reachable_through_the_bus() {
        let mut b = bus();
        b.write(0x6000_0000, Word, u32::from(b'U')).unwrap();
        b.write(0x6004_3000, Word, u32::from(b'J')).unwrap();
        b.write(0x6004_3004, Word, 1).unwrap();
        b.write(0x6001_0010, Word, 1).unwrap(); // UART1.INT_CLR, written by the ROM's uartAttach
        assert_eq!(b.periph.uart0.take_tx(), b"U");
        assert_eq!(b.periph.usb_serial_jtag.take_tx(), b"J");
        assert!(b.diag.drain().is_empty());
    }
```

- [ ] **Step 2: Lancer, constater l'échec de compilation**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implémenter**

`uart.rs` :

```rust
//! UART controller, transmit side. Register layout: `uart_reg.h` (report §G.2).
//!
//! Decision D7 of the SP1b-1 plan: a byte written to the FIFO leaves at once, so the transmit FIFO
//! always reads empty and the baud rate has no effect. Real timing, reception and interrupts arrive
//! in SP1b-3; until then the emulated boot is shorter than the real one by the time the UART would
//! take to send its log.
use crate::periph::Storage;
use emu_core::Peripheral;

const FIFO: u32 = 0x00;
const INT_ENA: u32 = 0x0C;
const INT_CLR: u32 = 0x10;
const CLKDIV: u32 = 0x14;
const STATUS: u32 = 0x1C;
const CONF0: u32 = 0x20;
const CONF1: u32 = 0x24;
const CLK_CONF: u32 = 0x78;
/// Configuration the ROM and ESP-IDF write and read back; nothing acts on it yet.
const CONFIG: [u32; 5] = [INT_ENA, CLKDIV, CONF0, CONF1, CLK_CONF];
/// `STATUS` bits [9:0] `RXFIFO_CNT` and [25:16] `TXFIFO_CNT`.
const FIFO_COUNTS: u32 = 0x03FF_03FF;

#[derive(Clone, Debug)]
pub struct Uart {
    /// `STATUS` with both FIFO counts at zero: the line-state bits keep their reset value.
    idle_status: u32,
    config: Storage,
    tx: Vec<u8>,
}

impl Uart {
    #[must_use]
    pub fn new(base: u32) -> Uart {
        let status_reset = svd_regs::esp32c3::lookup(base + STATUS).map_or(0, |r| r.reset_value);
        Uart { idle_status: status_reset & !FIFO_COUNTS, config: Storage::for_offsets(base, &CONFIG), tx: Vec::new() }
    }

    /// The bytes transmitted since the last call.
    pub fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }
}

impl Peripheral for Uart {
    fn read(&mut self, offset: u32) -> Option<u32> {
        match offset {
            STATUS => Some(self.idle_status),
            _ => self.config.read(offset),
        }
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            FIFO => {
                self.tx.push(value.to_le_bytes()[0]);
                true
            }
            // No interrupt condition is modelled yet, so there is nothing to clear.
            INT_CLR => true,
            _ => self.config.write(offset, value),
        }
    }
}
```

`usb_serial_jtag.rs` :

```rust
//! USB-Serial-JTAG, device-to-host serial path. Register layout: `usb_serial_jtag_reg.h` (report §G.3).
//!
//! The model behaves like a board with a host attached and reading: the IN endpoint always has
//! room. Reception, interrupts and host attach/detach arrive in SP1b-3.
use emu_core::Peripheral;

const EP1: u32 = 0x00;
const EP1_CONF: u32 = 0x04;
/// `EP1_CONF` bit 0, write 1: commit the bytes written so far as one IN packet.
const WR_DONE: u32 = 1 << 0;
/// `EP1_CONF` bit 1, read-only: the IN endpoint can take a byte.
const SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
/// Size of a full-speed bulk packet: the hardware sends a full FIFO without waiting for `WR_DONE`.
/// ASSUMPTION: taken from ESP-IDF's driver comments (`usb_serial_jtag_ll_txfifo_flush`), not measured.
const PACKET_BYTES: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct UsbSerialJtag {
    packet: Vec<u8>,
    tx: Vec<u8>,
}

impl UsbSerialJtag {
    #[must_use]
    pub const fn new() -> UsbSerialJtag {
        UsbSerialJtag { packet: Vec::new(), tx: Vec::new() }
    }

    /// The bytes sent to the host since the last call.
    pub fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.tx)
    }

    fn commit(&mut self) {
        self.tx.append(&mut self.packet);
    }
}

impl Peripheral for UsbSerialJtag {
    fn read(&mut self, offset: u32) -> Option<u32> {
        (offset == EP1_CONF).then_some(SERIAL_IN_EP_DATA_FREE)
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            EP1 => {
                self.packet.push(value.to_le_bytes()[0]);
                if self.packet.len() == PACKET_BYTES {
                    self.commit();
                }
                true
            }
            EP1_CONF => {
                if value & WR_DONE != 0 {
                    self.commit();
                }
                true
            }
            _ => false,
        }
    }
}
```

Dans `periph/mod.rs` : deux `Uart` (`Uart::new(UART0.0)`, `Uart::new(UART1.0)`), un `UsbSerialJtag`, leurs plages et leurs branches dans `device`.

- [ ] **Step 4: Lancer les tests**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"; cargo clippy -p soc-esp32c3 --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"`
Expected: `exit=0`, `clippy=0`.

- [ ] **Step 5: Commit**

```bash
/usr/bin/git add crates/soc-esp32c3
/usr/bin/git commit -m "feat(soc-esp32c3): add the UART and USB-Serial-JTAG transmit paths" -- crates/soc-esp32c3
```

---

### Task 8: La machine `Esp32C3` et le démarrage de la vraie ROM

**Files:**
- Create: `crates/soc-esp32c3/src/machine.rs`
- Modify: `crates/soc-esp32c3/src/lib.rs`, `crates/soc-esp32c3/tests/real_rom.rs`

**Interfaces:**
- Consumes: `SocBus`, `SocConfig`, `RomImage`, `Diagnostic`, `cpu_rv32::{Hart, HartConfig, MisalignedPolicy, VendorCsrs, StepOutcome, DatasheetTiming, Exception}`, `emu_core::{Cycles, TraceHash}`.
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum ConsoleChannel { Uart0, UsbSerialJtag }
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum StopReason {
      CycleBudget,             // la limite de cycles demandée est atteinte
      StrictMmio(Diagnostic),  // accès non émulé en mode strict
      Idle,                    // le CPU attend une interruption que rien ne peut encore lever
  }
  pub struct Esp32C3 { /* privé */ }
  impl Esp32C3 {
      pub fn new(rom: RomImage, config: &SocConfig) -> Esp32C3;
      pub fn run_until(&mut self, limit: Cycles) -> StopReason;   // `limit` : valeur absolue de `cycles()`
      pub fn cycles(&self) -> Cycles;
      pub fn instructions(&self) -> u64;
      pub fn pc(&self) -> u32;
      pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8>;
      pub fn drain_diagnostics(&mut self) -> Vec<Diagnostic>;
      pub fn trace_hash(&self) -> Option<u64>;                    // Some si SocConfig::trace_hash
  }
  ```
  `lib.rs` ré-exporte `Esp32C3`, `ConsoleChannel`, `StopReason`, `SocConfig`, `ChipRevision`, `RomImage`, `RomError`, `Diagnostic`.

- [ ] **Step 1: Écrire les tests sur des ROM écrites à la main (`machine.rs`, module `tests`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::memmap::{ROM_BASE, ROM_SIZE};
    use cpu_rv32::asm::{addi, csrrs, csrrwi, jal, lui, lw, sw, wfi};

    const T0: u8 = 5;
    const T1: u8 = 6;
    /// `j .`: the program parks itself.
    const HANG: u32 = 0x0000_006f;

    fn machine_with(program: &[u32], config: SocConfig) -> Esp32C3 {
        let mut rom = vec![0u8; ROM_SIZE as usize];
        for (i, word) in program.iter().enumerate() {
            rom[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes());
        }
        Esp32C3::new(RomImage::from_flat(rom).unwrap(), &config)
    }
    fn machine(program: &[u32]) -> Esp32C3 {
        machine_with(program, SocConfig::default())
    }
    fn print(base: u32, text: &[u8]) -> Vec<u32> {
        let mut program = vec![lui(T0, base)];
        for &byte in text {
            program.extend([addi(T1, 0, i32::from(byte)), sw(T1, T0, 0)]);
        }
        program
    }

    #[test]
    fn hang_instruction_is_what_the_assembler_produces() {
        assert_eq!(jal(0, 0), HANG);
    }

    #[test]
    fn starts_at_the_reset_vector_and_prints_through_uart0() {
        let mut program = print(0x6000_0000, b"Hi");
        program.push(HANG);
        let mut m = machine(&program);
        assert_eq!(m.pc(), ROM_BASE);
        assert_eq!(m.run_until(Cycles(500)), StopReason::CycleBudget);
        assert_eq!(m.take_console(ConsoleChannel::Uart0), b"Hi");
        assert!(m.take_console(ConsoleChannel::Uart0).is_empty());
        assert!(m.cycles().get() >= 500);
        assert!(m.drain_diagnostics().is_empty());
    }

    #[test]
    fn usb_serial_jtag_console_is_a_separate_channel() {
        let mut program = print(0x6004_3000, b"ok");
        program.extend([addi(T1, 0, 1), sw(T1, T0, 4), HANG]);
        let mut m = machine(&program);
        m.run_until(Cycles(500));
        assert_eq!(m.take_console(ConsoleChannel::UsbSerialJtag), b"ok");
        assert!(m.take_console(ConsoleChannel::Uart0).is_empty());
    }

    #[test]
    fn budget_is_absolute_so_a_run_can_be_resumed() {
        let mut m = machine(&[HANG]);
        assert_eq!(m.run_until(Cycles(100)), StopReason::CycleBudget);
        let first = m.cycles().get();
        assert_eq!(m.run_until(Cycles(100)), StopReason::CycleBudget, "already past the limit: nothing runs");
        assert_eq!(m.cycles().get(), first);
        m.run_until(Cycles(300));
        assert!(m.cycles().get() >= 300);
    }

    #[test]
    fn the_rom_delay_counter_is_available() {
        // What ets_delay_us relies on: enable through 0x800/0x801, then watch 0x802 advance.
        let mut m = machine(&[csrrwi(0, 0x800, 1), csrrwi(0, 0x801, 1), csrrs(T0, 0x802, 0), csrrs(T1, 0x802, 0), HANG]);
        m.run_until(Cycles(200));
        assert!(m.drain_diagnostics().is_empty(), "no illegal-instruction exception");
    }

    #[test]
    fn unmodeled_access_is_reported_with_the_pc_of_the_instruction() {
        let ledc = svd_regs::esp32c3::registers().iter().find(|r| r.peripheral == "LEDC").unwrap();
        let mut m = machine(&[lui(T0, ledc.addr), lw(T1, T0, 0), HANG]);
        assert_eq!(m.run_until(Cycles(200)), StopReason::CycleBudget);
        let listed = m.drain_diagnostics();
        assert!(matches!(listed[..], [Diagnostic::UnmodeledMmio { pc, .. }] if pc == ROM_BASE + 4), "{listed:?}");
    }

    #[test]
    fn strict_mode_stops_at_the_unmodeled_access() {
        let ledc = svd_regs::esp32c3::registers().iter().find(|r| r.peripheral == "LEDC").unwrap();
        let config = SocConfig { strict_mmio: true, ..SocConfig::default() };
        let mut m = machine_with(&[lui(T0, ledc.addr), lw(T1, T0, 0), HANG], config);
        let StopReason::StrictMmio(diagnostic) = m.run_until(Cycles(10_000)) else { panic!("expected a strict stop") };
        assert!(diagnostic.to_string().contains("LEDC"), "{diagnostic}");
        assert!(m.cycles().get() < 100, "stopped right after the access");
    }

    #[test]
    fn wfi_with_nothing_to_wake_it_stops_as_idle() {
        let mut m = machine(&[wfi(), HANG]);
        assert_eq!(m.run_until(Cycles(10_000)), StopReason::Idle);
    }

    #[test]
    fn exception_loop_is_reported_once_per_site() {
        // An all-zero word is an illegal instruction; mtvec is 0, so the handler address is unmapped
        // and the hart then faults on every fetch at pc 0.
        let mut m = machine(&[0]);
        assert_eq!(m.run_until(Cycles(5_000)), StopReason::CycleBudget);
        let exceptions: Vec<_> = m.drain_diagnostics().into_iter().filter(|d| matches!(d, Diagnostic::Exception { .. })).collect();
        assert_eq!(exceptions.len(), 2, "{exceptions:?}");
        assert!(matches!(exceptions[0], Diagnostic::Exception { cause: Exception::IllegalInstr, pc, .. } if pc == ROM_BASE));
        assert!(matches!(exceptions[1], Diagnostic::Exception { cause: Exception::InstrAccessFault, pc: 0, .. }));
    }

    #[test]
    fn same_program_gives_the_same_trace_hash_and_another_program_a_different_one() {
        let config = SocConfig { trace_hash: true, ..SocConfig::default() };
        let run = |program: &[u32]| {
            let mut m = machine_with(program, config);
            m.run_until(Cycles(2_000));
            (m.trace_hash().unwrap(), m.instructions(), m.cycles())
        };
        let a = print(0x6000_0000, b"A");
        let b = print(0x6000_0000, b"AB");
        assert_eq!(run(&a), run(&a));
        assert_ne!(run(&a).0, run(&b).0);
        assert_eq!(machine(&a).trace_hash(), None, "off unless asked for");
    }
}
```

- [ ] **Step 2: Lancer, constater l'échec de compilation**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implémenter `machine.rs`**

```rust
//! The ESP32-C3 as a machine: one hart, its bus, and the loop that runs them.
use crate::bus::SocBus;
use crate::config::SocConfig;
use crate::diag::Diagnostic;
use crate::memmap::RESET_PC;
use crate::rom::RomImage;
use cpu_rv32::{DatasheetTiming, Exception, Hart, HartConfig, MisalignedPolicy, StepOutcome, VendorCsrs};
use emu_core::{Cycles, TraceHash};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleChannel {
    Uart0,
    UsbSerialJtag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The cycle limit passed to [`Esp32C3::run_until`] was reached.
    CycleBudget,
    /// `--strict-mmio`: the program touched a register the emulator does not model.
    StrictMmio(Diagnostic),
    /// The CPU is in `wfi` and nothing can raise an interrupt yet (interrupt sources arrive in SP1b-3).
    Idle,
}

#[derive(Debug)]
pub struct Esp32C3 {
    hart: Hart,
    bus: SocBus,
    timing: DatasheetTiming,
    trace: Option<TraceHash>,
}

impl Esp32C3 {
    /// A machine in its power-on reset state, about to execute the first ROM instruction.
    #[must_use]
    pub fn new(rom: RomImage, config: &SocConfig) -> Esp32C3 {
        // Misaligned loads and stores trap: the conservative reading of the hardware documentation
        // (report §B.1); the QEMU lockstep of SP1b-4 arbitrates.
        let hart_config = HartConfig { misaligned: MisalignedPolicy::Trap, vendor_csrs: VendorCsrs::Esp32C3 };
        Esp32C3 {
            hart: Hart::new(RESET_PC, hart_config),
            bus: SocBus::new(rom, config),
            timing: DatasheetTiming,
            trace: config.trace_hash.then(TraceHash::new),
        }
    }

    /// Runs until the hart's cycle counter reaches `limit` or the machine stops for another reason.
    pub fn run_until(&mut self, limit: Cycles) -> StopReason {
        while self.hart.cycles.get() < limit.get() {
            let pc = self.hart.pc;
            self.bus.set_pc(pc);
            match self.hart.step(&mut self.bus, &self.timing) {
                StepOutcome::Waiting => return StopReason::Idle,
                // An `ecall` is a request to the trap handler, not a fault worth reporting.
                StepOutcome::Trapped { trap, .. } if trap.cause != Exception::EcallM => {
                    self.bus.diag.record(Diagnostic::Exception { cause: trap.cause, tval: trap.tval, pc });
                }
                _ => {}
            }
            if let Some(trace) = &mut self.trace {
                trace.mix_u32(pc);
            }
            if let Some(violation) = self.bus.diag.take_violation() {
                return StopReason::StrictMmio(violation);
            }
        }
        StopReason::CycleBudget
    }

    /// CPU clock cycles elapsed since reset.
    #[must_use]
    pub const fn cycles(&self) -> Cycles { self.hart.cycles }
    #[must_use]
    pub const fn instructions(&self) -> u64 { self.hart.instret }
    #[must_use]
    pub const fn pc(&self) -> u32 { self.hart.pc }

    /// The bytes the firmware sent on `channel` since the last call.
    pub fn take_console(&mut self, channel: ConsoleChannel) -> Vec<u8> {
        match channel {
            ConsoleChannel::Uart0 => self.bus.periph.uart0.take_tx(),
            ConsoleChannel::UsbSerialJtag => self.bus.periph.usb_serial_jtag.take_tx(),
        }
    }

    pub fn drain_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.bus.diag.drain()
    }

    /// Fingerprint of the executed PC sequence, when [`SocConfig::trace_hash`] is set.
    #[must_use]
    pub fn trace_hash(&self) -> Option<u64> {
        self.trace.as_ref().map(TraceHash::finish)
    }
}
```

Adapter aux signatures réelles de `TraceHash` (`new`, `mix_u32`, `finish`) et de `Cycles` (comparaison par `get()`). Si `DatasheetTiming` s'instancie autrement que par son littéral d'unité, utiliser `DatasheetTiming::default()`.

- [ ] **Step 4: Lancer les tests unitaires**

Run: `cargo test -p soc-esp32c3 > /tmp/soc.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

- [ ] **Step 5: Ajouter les tests de démarrage de la vraie ROM (`tests/real_rom.rs`)**

```rust
use emu_core::Cycles;
use soc_esp32c3::{ConsoleChannel, Diagnostic, Esp32C3, SocConfig, StopReason};

/// Far more than the ROM needs to reach its banner; a bound, not an expectation.
const BANNER_BUDGET: u64 = 20_000_000;
const SLICE: u64 = 100_000;
const BOOT_LINE: &str = "rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n";

struct Boot {
    uart0: String,
    usb: String,
    diagnostics: Vec<Diagnostic>,
    stop: StopReason,
    trace_hash: Option<u64>,
    cycles: u64,
}

/// Runs the ROM of `revision` until its boot line is complete on UART0, or the budget runs out.
/// Carriage returns are dropped: the ROM expands `\n` to `\r\n` on the UART.
fn boot(revision: u16, config: SocConfig) -> Boot {
    let mut machine = Esp32C3::new(RomImage::from_elf(&elf(revision)).unwrap(), &config);
    let (mut uart0, mut usb, mut diagnostics) = (Vec::new(), Vec::new(), Vec::new());
    let mut stop = StopReason::CycleBudget;
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\r', "");
    while stop == StopReason::CycleBudget && machine.cycles().get() < BANNER_BUDGET && !text(&uart0).contains(BOOT_LINE) {
        stop = machine.run_until(Cycles(machine.cycles().get() + SLICE));
        uart0.extend(machine.take_console(ConsoleChannel::Uart0));
        usb.extend(machine.take_console(ConsoleChannel::UsbSerialJtag));
        diagnostics.extend(machine.drain_diagnostics());
    }
    Boot { uart0: text(&uart0), usb: text(&usb), diagnostics, stop, trace_hash: machine.trace_hash(), cycles: machine.cycles().get() }
}

fn banner(rom: &str, build: &str) -> String {
    format!("ESP-ROM:{rom}\nBuild:{build}\n{BOOT_LINE}")
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_rom_prints_its_banner_on_uart0() {
    let boot = boot(3, SocConfig::default());
    assert!(boot.uart0.starts_with(&banner("esp32c3-api1-20210207", "Feb  7 2021")), "UART0 printed: {:?}\ndiagnostics: {:#?}", boot.uart0, boot.diagnostics);
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_rom_prints_the_same_banner_on_usb_serial_jtag() {
    let boot = boot(3, SocConfig::default());
    assert!(boot.usb.starts_with(&banner("esp32c3-api1-20210207", "Feb  7 2021")), "USB printed: {:?}", boot.usb);
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev0_and_rev101_roms_print_their_own_banners() {
    assert!(boot(0, SocConfig::default()).uart0.starts_with(&banner("esp32c3-20200918", "Sep 18 2020")));
    assert!(boot(101, SocConfig::default()).uart0.starts_with(&banner("esp32c3-eco7-20230720", "Jul 20 2023")));
}

/// Everything the ROM touches up to its banner is modelled: in strict mode the banner is complete
/// before the machine stops, and no CPU exception occurred on the way.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn rev3_banner_is_reached_without_any_unmodeled_access_or_exception() {
    let boot = boot(3, SocConfig { strict_mmio: true, ..SocConfig::default() });
    assert!(boot.uart0.starts_with(&banner("esp32c3-api1-20210207", "Feb  7 2021")), "stopped early: {:?}\nUART0: {:?}", boot.stop, boot.uart0);
    assert!(!boot.diagnostics.iter().any(|d| matches!(d, Diagnostic::Exception { .. })), "{:#?}", boot.diagnostics);
}

#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn two_boots_are_identical_down_to_the_trace_hash() {
    let config = SocConfig { trace_hash: true, ..SocConfig::default() };
    let (a, b) = (boot(3, config), boot(3, config));
    assert!(a.trace_hash.is_some());
    assert_eq!((a.trace_hash, a.cycles, &a.uart0, &a.diagnostics), (b.trace_hash, b.cycles, &b.uart0, &b.diagnostics));
}
```

(`use soc_esp32c3::rom::RomImage;` et la fonction `elf` existent déjà dans ce fichier depuis la Task 4.)

- [ ] **Step 6: Démarrer la vraie ROM**

Run: `INKEMU_ROM_ELF_DIR=<répertoire des ELF> cargo test -p soc-esp32c3 --test real_rom -- --ignored > /tmp/real.log 2>&1; echo "exit=$?"`
Expected: `exit=0`, 7 tests.

C'est l'étape de mise au point du plan : la ROM réelle peut toucher un registre que le rapport n'a pas listé. Règles :

1. Lire l'échec : le message affiche ce que l'UART a reçu, la raison de l'arrêt et les diagnostics. Un diagnostic `UnmodeledMmio` nomme le registre et le PC ; `Exception` nomme la cause et le PC.
2. Situer le PC dans la ROM : `docker run --rm --entrypoint riscv64-unknown-elf-addr2line -v <répertoire des ELF>:/w:ro inkemu/riscv-gcc -f -e /w/esp32c3_rev3_rom.elf <pc>` donne le symbole ; `riscv64-unknown-elf-objdump -d --start-address=<a> --stop-address=<b>` montre le code. Les ELF sont des données non fiables : les lire seulement, dans Docker, montés en lecture seule.
3. Modéliser ce qui manque **dans le périphérique concerné**, avec un test unitaire et la source en commentaire (rapport §J, symbole de ROM, en-tête ESP-IDF). Un registre de configuration sans effet avant la bannière rejoint la liste `Storage` de son périphérique, avec une phrase qui dit pourquoi.
4. Interdits : modifier ou patcher l'image de ROM ; tester un PC ou un symbole de ROM dans le code de l'émulateur ; faire passer un test en assouplissant son assertion ; marquer un registre « modélisé » en renvoyant une valeur choisie pour débloquer sans source.
5. Si la ROM attend indéfiniment une condition qui dépend du temps (au-delà du compteur `0x802`), s'arrêter et le signaler : c'est une information de conception pour 1b-3, pas un détail à contourner.
6. Consigner chaque registre ajouté par rapport à ce plan dans le rapport de tâche.

- [ ] **Step 7: Commit**

```bash
/usr/bin/git add crates/soc-esp32c3
/usr/bin/git commit -m "feat(soc-esp32c3): add the machine loop and boot the real mask ROM to its banner" -- crates/soc-esp32c3
```

---
### Task 9: Cache de ROM : manifeste, téléchargement vérifié, extraction

**Files:**
- Create: `crates/emu-cli/src/rom_store.rs`
- Modify: `crates/emu-cli/src/lib.rs`, `crates/emu-cli/Cargo.toml`, `Cargo.toml` (dépendances `ureq`, `flate2`, `tar`, `directories`), `deny.toml` (seulement si `cargo deny` l'exige, voir Step 5)

**Interfaces:**
- Consumes: `soc_esp32c3::{RomImage, RomError, ChipRevision}`, `soc_esp32c3::test_support::rom_elf` (tests).
- Produces:
  ```rust
  pub struct RomArchive { pub release: &'static str, pub url: &'static str, pub sha256: &'static str, pub size: u64 }
  pub struct RomEntry { pub rom_revision: u16, pub elf_name: &'static str, pub elf_sha256: &'static str, pub image_sha256: &'static str }
  pub const ESP_ROM_ELFS: RomArchive;
  pub const ESP32C3_ROMS: [RomEntry; 3];                    // rev0, rev3, rev101
  pub fn rom_entry_for(revision: ChipRevision) -> &'static RomEntry;   // plus haute ROM ≤ révision de la puce
  pub trait Fetch { fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String>; }
  pub struct HttpFetch;                                     // impl Fetch, via ureq
  pub enum RomSource<'a> { Cache { dir: PathBuf, fetch: &'a dyn Fetch }, Directory(PathBuf) }
  pub fn default_cache_dir() -> Result<PathBuf, RomStoreError>;       // <data_dir>/inkemu/rom
  pub fn load_elf(source: &RomSource<'_>, archive: &RomArchive, entry: &RomEntry) -> Result<Vec<u8>, RomStoreError>;
  pub fn load_rom(source: &RomSource<'_>, archive: &RomArchive, entry: &RomEntry) -> Result<RomImage, RomStoreError>;
  pub enum RomStoreError { Download, ArchiveMismatch, MissingInArchive, ElfMismatch, NotInDirectory, ImageMismatch, Io, NoDataDir, Rom }
  ```

Règles (ADR 0001, « Conséquences ») : l'empreinte de l'archive est vérifiée **avant** toute extraction ; aucun chemin fourni par l'archive n'est écrit sur disque (seule l'entrée dont le nom est exactement celui attendu est lue, en mémoire) ; l'écriture se fait dans un fichier temporaire puis par renommage ; un fichier du cache dont l'empreinte diffère n'est jamais chargé ni remplacé en silence ; `RomSource::Directory` ne télécharge jamais.

Manifeste (valeurs relevées dans `docs/research/esp32c3-rom-elfs.md`, §2 et §9) :

| ROM | Fichier | SHA-256 de l'ELF | SHA-256 de l'image |
|---|---|---|---|
| rev0 | `esp32c3_rev0_rom.elf` | `11d565922e0a405b477756de748a14046f86dfb40d997766252a04cb378f0694` | `832b9a17b4bca5fe307d2d84b3fcd9cdc75f6a8a51956a3e114b4d6713710565` |
| rev3 | `esp32c3_rev3_rom.elf` | `19ac22e08707df926fb0cf4c54795d4067b983fae8635f396ded173a6d78fc3c` | `0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99` |
| rev101 | `esp32c3_rev101_rom.elf` | `9495e1453f36f7eec4112714ea1dca22d77808776adffa93dda7dda4393e8336` | `8b41b9b114e110e373a58188e6e86b044719f3b4e6da0441c6e8b68b72986651` |

Archive : release `20260528`, URL `https://github.com/espressif/esp-rom-elfs/releases/download/20260528/esp-rom-elfs-20260528.tar.gz`, SHA-256 `caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46`, 4 902 355 octets.

- [ ] **Step 1: Écrire les tests (`rom_store.rs`, module `tests`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::cell::Cell;
    use std::io::Write as _;

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }
    fn leak(text: String) -> &'static str {
        Box::leak(text.into_boxed_str())
    }

    /// A gzip-compressed tar archive holding `files`.
    fn archive_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            // `append_data` would refuse `..`: write the raw name so the hostile case stays testable.
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            tar.append(&header, *bytes).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    struct FakeFetch {
        body: Result<Vec<u8>, String>,
        calls: Cell<u32>,
    }
    impl FakeFetch {
        fn serving(body: Vec<u8>) -> FakeFetch {
            FakeFetch { body: Ok(body), calls: Cell::new(0) }
        }
    }
    impl Fetch for FakeFetch {
        fn fetch(&self, _url: &str, _max_bytes: u64) -> Result<Vec<u8>, String> {
            self.calls.set(self.calls.get() + 1);
            self.body.clone()
        }
    }

    const ELF_NAME: &str = "esp32c3_rev3_rom.elf";

    /// An archive, and the manifest that pins it and the tiny valid ROM ELF inside.
    fn fixture() -> (Vec<u8>, RomArchive, RomEntry) {
        let elf = soc_esp32c3::test_support::rom_elf().build();
        let image = RomImage::from_elf(&elf).unwrap();
        let body = archive_of(&[("esp32s3_rev0_rom.elf", b"other chip"), (ELF_NAME, &elf)]);
        let archive = RomArchive { release: "test", url: "https://example.invalid/roms.tar.gz", sha256: leak(sha(&body)), size: body.len() as u64 };
        let entry = RomEntry { rom_revision: 3, elf_name: ELF_NAME, elf_sha256: leak(sha(&elf)), image_sha256: leak(sha(image.bytes())) };
        (body, archive, entry)
    }

    fn files_in(dir: &std::path::Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut names: Vec<String> = entries.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn downloads_once_then_serves_from_the_cache() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("rom");
        let fetch = FakeFetch::serving(body);
        let source = RomSource::Cache { dir: cache.clone(), fetch: &fetch };
        let first = load_rom(&source, &archive, &entry).unwrap();
        let second = load_rom(&source, &archive, &entry).unwrap();
        assert_eq!(first, second);
        assert_eq!(fetch.calls.get(), 1, "the second load reads the cache");
        assert_eq!(files_in(&cache), [ELF_NAME], "only the requested ELF is stored, no temporary file left");
    }

    #[test]
    fn archive_hash_mismatch_is_refused_and_nothing_is_written() {
        let (mut body, archive, entry) = fixture();
        *body.last_mut().unwrap() ^= 1;
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body.clone());
        let source = RomSource::Cache { dir: dir.path().to_path_buf(), fetch: &fetch };
        let err = load_elf(&source, &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::ArchiveMismatch { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains(archive.sha256) && text.contains(&sha(&body)) && text.contains("--rom-dir"), "{text}");
        assert!(files_in(dir.path()).is_empty());
    }

    #[test]
    fn truncated_or_oversized_download_is_refused() {
        let (body, archive, entry) = fixture();
        for served in [body[..body.len() / 2].to_vec(), [body.clone(), vec![0; 16]].concat()] {
            let dir = tempfile::tempdir().unwrap();
            let fetch = FakeFetch::serving(served);
            let source = RomSource::Cache { dir: dir.path().to_path_buf(), fetch: &fetch };
            assert!(matches!(load_elf(&source, &archive, &entry), Err(RomStoreError::ArchiveMismatch { .. })));
            assert!(files_in(dir.path()).is_empty());
        }
    }

    #[test]
    fn tampered_cached_file_is_refused_with_both_hashes() {
        let (body, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let tampered = b"not the ROM".to_vec();
        std::fs::write(dir.path().join(ELF_NAME), &tampered).unwrap();
        let fetch = FakeFetch::serving(body);
        let source = RomSource::Cache { dir: dir.path().to_path_buf(), fetch: &fetch };
        let err = load_elf(&source, &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::ElfMismatch { .. }), "{err}");
        let text = err.to_string();
        assert!(text.contains(ELF_NAME) && text.contains(entry.elf_sha256) && text.contains(&sha(&tampered)), "{text}");
        assert_eq!(fetch.calls.get(), 0, "a suspicious file is never silently replaced");
        assert_eq!(std::fs::read(dir.path().join(ELF_NAME)).unwrap(), tampered, "and never deleted either");
    }

    #[test]
    fn download_failure_explains_the_offline_route() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch { body: Err("dns error: no such host".into()), calls: Cell::new(0) };
        let source = RomSource::Cache { dir: dir.path().to_path_buf(), fetch: &fetch };
        let text = load_elf(&source, &archive, &entry).unwrap_err().to_string();
        for part in [archive.url, "dns error", ELF_NAME, "--rom-dir"] {
            assert!(text.contains(part), "{part} missing from: {text}");
        }
        assert!(files_in(dir.path()).is_empty());
    }

    #[test]
    fn explicit_directory_never_downloads() {
        let (_, archive, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        let err = load_elf(&RomSource::Directory(dir.path().to_path_buf()), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::NotInDirectory { .. }), "{err}");
        assert!(err.to_string().contains(ELF_NAME) && err.to_string().contains("esp-rom-elfs"), "{err}");
        std::fs::write(dir.path().join(ELF_NAME), soc_esp32c3::test_support::rom_elf().build()).unwrap();
        assert!(load_rom(&RomSource::Directory(dir.path().to_path_buf()), &archive, &entry).is_ok());
    }

    #[test]
    fn only_the_exact_entry_name_is_extracted() {
        let elf = soc_esp32c3::test_support::rom_elf().build();
        let body = archive_of(&[("../esp32c3_rev3_rom.elf", b"escape"), ("sub/esp32c3_rev3_rom.elf", b"nested"), (ELF_NAME, &elf)]);
        let archive = RomArchive { release: "test", url: "https://example.invalid/a.tar.gz", sha256: leak(sha(&body)), size: body.len() as u64 };
        let entry = RomEntry { rom_revision: 3, elf_name: ELF_NAME, elf_sha256: leak(sha(&elf)), image_sha256: "" };
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let fetch = FakeFetch::serving(body);
        assert_eq!(load_elf(&RomSource::Cache { dir: cache.clone(), fetch: &fetch }, &archive, &entry).unwrap(), elf);
        assert_eq!(files_in(&cache), [ELF_NAME]);
        assert_eq!(files_in(root.path()), ["cache"], "nothing escaped the cache directory");
    }

    #[test]
    fn missing_entry_and_wrong_entry_content_are_reported() {
        let (_, _, entry) = fixture();
        let dir = tempfile::tempdir().unwrap();
        for (files, expect_missing) in [(vec![("esp32s3_rev0_rom.elf", &b"x"[..])], true), (vec![(ELF_NAME, &b"wrong bytes"[..])], false)] {
            let body = archive_of(&files);
            let archive = RomArchive { release: "test", url: "https://example.invalid/a.tar.gz", sha256: leak(sha(&body)), size: body.len() as u64 };
            let fetch = FakeFetch::serving(body);
            let err = load_elf(&RomSource::Cache { dir: dir.path().to_path_buf(), fetch: &fetch }, &archive, &entry).unwrap_err();
            assert_eq!(matches!(err, RomStoreError::MissingInArchive { .. }), expect_missing, "{err}");
            assert_eq!(matches!(err, RomStoreError::ElfMismatch { .. }), !expect_missing, "{err}");
            assert!(files_in(dir.path()).is_empty());
        }
    }

    #[test]
    fn image_hash_is_checked_after_conversion() {
        let (body, archive, mut entry) = fixture();
        entry.image_sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let dir = tempfile::tempdir().unwrap();
        let fetch = FakeFetch::serving(body);
        let err = load_rom(&RomSource::Cache { dir: dir.path().to_path_buf(), fetch: &fetch }, &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::ImageMismatch { .. }), "{err}");
    }

    #[test]
    fn a_file_that_is_not_a_rom_reports_the_conversion_error_with_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"\x7fELF but not really".to_vec();
        std::fs::write(dir.path().join(ELF_NAME), &bytes).unwrap();
        let archive = RomArchive { release: "test", url: "", sha256: "", size: 0 };
        let entry = RomEntry { rom_revision: 3, elf_name: ELF_NAME, elf_sha256: leak(sha(&bytes)), image_sha256: "" };
        let err = load_rom(&RomSource::Directory(dir.path().to_path_buf()), &archive, &entry).unwrap_err();
        assert!(matches!(err, RomStoreError::Rom { .. }), "{err}");
        assert!(err.to_string().contains(ELF_NAME), "{err}");
    }

    #[test]
    fn rom_entry_follows_the_next_lower_revision_rule() {
        let rom_for = |text: &str| rom_entry_for(text.parse().unwrap()).rom_revision;
        assert_eq!((rom_for("v0.0"), rom_for("v0.2")), (0, 0));
        assert_eq!((rom_for("v0.3"), rom_for("v0.4"), rom_for("v1.0")), (3, 3, 3));
        assert_eq!((rom_for("v1.1"), rom_for("v3.15")), (101, 101));
    }

    #[test]
    fn manifest_is_well_formed() {
        let hex64 = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        assert!(hex64(ESP_ROM_ELFS.sha256) && ESP_ROM_ELFS.url.starts_with("https://github.com/espressif/esp-rom-elfs/releases/download/"));
        assert!(ESP_ROM_ELFS.url.contains(ESP_ROM_ELFS.release));
        for (entry, revision) in ESP32C3_ROMS.iter().zip([0, 3, 101]) {
            assert_eq!(entry.rom_revision, revision);
            assert_eq!(entry.elf_name, format!("esp32c3_rev{revision}_rom.elf"));
            assert!(hex64(entry.elf_sha256) && hex64(entry.image_sha256));
        }
        // Dummy use so an unused import does not hide a refactoring mistake.
        std::io::sink().write_all(b"").unwrap();
    }
}
```

(Retirer la dernière ligne et l'import `Write` s'ils ne servent à rien dans l'implémentation finale des tests.)

- [ ] **Step 2: Lancer, constater l'échec de compilation**

Run: `cargo test -p emu-cli rom_store > /tmp/cli.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 3: Implémenter `rom_store.rs`**

Dépendances de `emu-cli` : `soc-esp32c3`, `sha2` (passe de dev-dépendance à dépendance), `ureq`, `flate2`, `tar`, `directories` ; en dev-dépendance `soc-esp32c3 = { workspace = true, features = ["test-support"] }`. Versions : la dernière stable de chacune (`cargo info`). Pour `ureq`, garder TLS par `rustls` et désactiver ce qui ne sert pas (cookies, JSON, proxys SOCKS).

Messages d'erreur (cause ; correction) :

```rust
#[derive(Debug, thiserror::Error)]
pub enum RomStoreError {
    #[error("cannot download {url}: {detail}; check the network connection, or fetch that archive on another machine, extract `{elf_name}` and pass its directory with --rom-dir")]
    Download { url: String, detail: String, elf_name: &'static str },
    #[error("{url} does not match the pinned archive (expected sha256 {expected}, got {found}); retry the download, or supply the ROM yourself with --rom-dir")]
    ArchiveMismatch { url: String, expected: &'static str, found: String },
    #[error("the archive {url} holds no `{elf_name}`; the pinned release changed layout, report it and supply the ROM with --rom-dir meanwhile")]
    MissingInArchive { url: String, elf_name: &'static str },
    #[error("{path} is not the expected ROM (expected sha256 {expected}, got {found}); delete it to download it again, or point --rom-dir at a directory holding the genuine file")]
    ElfMismatch { path: String, expected: &'static str, found: String },
    #[error("{path} not found; --rom-dir must point at a directory holding the ROM ELF files of esp-rom-elfs release {release}")]
    NotInDirectory { path: String, release: &'static str },
    #[error("the ROM image built from {path} has sha256 {found}, expected {expected}; this is a bug in inkemu's ROM conversion, please report it")]
    ImageMismatch { path: String, expected: &'static str, found: String },
    #[error("cannot {action} {path}: {detail}; check the permissions of the ROM directory or choose another one with --rom-dir")]
    Io { action: &'static str, path: String, detail: String },
    #[error("this platform has no user data directory; pass the ROM directory explicitly with --rom-dir")]
    NoDataDir,
    #[error("{path}: {source}")]
    Rom { path: String, source: RomError },
}
```

Déroulé de `load_elf` :

1. `path = dir.join(entry.elf_name)`.
2. Si `path` existe : lire le fichier. Sinon, pour `Directory` → `NotInDirectory` ; pour `Cache` → `download(...)` ci-dessous, qui renvoie les octets **après** les avoir écrits.
3. Comparer le SHA-256 des octets à `entry.elf_sha256` → `ElfMismatch` sinon. Le fichier n'est ni supprimé ni remplacé.

`download` :

1. `fetch.fetch(archive.url, archive.size)` → `Download` en cas d'échec.
2. Calculer le SHA-256 du corps ; si la longueur diffère de `archive.size` ou si l'empreinte diffère de `archive.sha256` → `ArchiveMismatch` (l'empreinte observée est celle du corps reçu). **Rien n'est décompressé avant ce contrôle.**
3. Parcourir `tar::Archive::new(flate2::read::GzDecoder::new(&body[..])).entries()` ; ne retenir que l'entrée dont `entry.path()` est exactement `Path::new(entry.elf_name)` (une seule composante normale) ; refuser une taille d'entrée supérieure à `MAX_ELF_BYTES` (4 Mio) ; lire son contenu en mémoire avec `take(MAX_ELF_BYTES)`. Une erreur de lecture de l'archive → `ArchiveMismatch` n'est pas adapté : utiliser `Io { action: "read the archive from", path: url, … }`. Absente → `MissingInArchive`.
4. Vérifier le SHA-256 du contenu contre `entry.elf_sha256` → `ElfMismatch` avec `path` = le chemin de destination (rien n'est écrit).
5. `create_dir_all(dir)`, écrire dans `dir/.<elf_name>.<pid>.tmp`, `rename` vers `path` ; en cas d'échec, supprimer le fichier temporaire et renvoyer `Io`.

`load_rom` : `load_elf`, puis `RomImage::from_elf` (→ `Rom` avec le chemin), puis SHA-256 de `image.bytes()` contre `entry.image_sha256` → `ImageMismatch`.

`rom_entry_for` : parmi `ESP32C3_ROMS`, la plus grande `rom_revision` inférieure ou égale à `revision.full()` (règle du README d'`esp-rom-elfs` : « use the next lower one ») ; rev0 convient toujours, d'où un `unwrap_or(&ESP32C3_ROMS[0])` sans panique possible.

`default_cache_dir` : `directories::ProjectDirs::from("", "", "inkemu")` → `data_dir().join("rom")`, `NoDataDir` si absent.

`HttpFetch` : requête GET avec `ureq`, lecture du corps bornée à `max_bytes + 1` octets (un corps plus long est détecté par le contrôle de taille), erreurs converties en texte. Aucun test unitaire ne touche le réseau : ce code est exercé par le job CI de la Task 12.

- [ ] **Step 4: Lancer les tests**

Run: `cargo test -p emu-cli > /tmp/cli.log 2>&1; echo "exit=$?"; cargo clippy -p emu-cli --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"`
Expected: `exit=0`, `clippy=0`.

- [ ] **Step 5: Contrôler les licences des nouvelles dépendances**

Run: `cargo deny check > /tmp/deny.log 2>&1; echo "exit=$?"; tail -30 /tmp/deny.log`
Expected: `exit=0`. Si une licence est refusée, ne pas élargir `allow` : ajouter une exception **par crate** dans `deny.toml`, par exemple pour les certificats racine de Mozilla embarqués par `webpki-roots` :

```toml
[[licenses.exceptions]]
# Mozilla's root certificate list: data under a permissive data licence, used only to verify TLS when downloading the ROM.
allow = ["CDLA-Permissive-2.0"]
crate = "webpki-roots"
```

Toute autre licence inattendue (copyleft, licence OpenSSL, licence inconnue) : s'arrêter et le signaler, avec le nom de la crate et la chaîne de dépendances (`cargo tree -i <crate>`).

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/emu-cli Cargo.toml Cargo.lock deny.toml
/usr/bin/git commit -m "feat(cli): add the pinned ROM manifest and the verified ROM cache" -- crates/emu-cli Cargo.toml Cargo.lock deny.toml
```

---

### Task 10: `boards` — profils d'appareil X4 et X3

**Files:**
- Create: `crates/boards/Cargo.toml`, `crates/boards/src/lib.rs`, `crates/boards/profiles/x4.toml`, `crates/boards/profiles/x3.toml`
- Modify: `Cargo.toml` (membre, `boards` dans `[workspace.dependencies]`, `toml` à sa dernière version stable)

**Interfaces:**
- Consumes: `soc_esp32c3::{ChipRevision, SocConfig, Esp32C3, RomImage}`.
- Produces:
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum SocFamily { Esp32C3 }
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct DeviceProfile { pub id: String, pub name: String, pub soc: SocFamily, pub chip_revision: ChipRevision }
  #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
  pub struct MachineOptions { pub strict_mmio: bool, pub trace_hash: bool }
  pub enum ProfileError { Parse { origin: String, detail: String }, UnknownDevice { id: String, known: String } }
  pub fn builtin_ids() -> &'static [&'static str];                       // ["x3", "x4"]
  pub fn builtin(id: &str) -> Result<DeviceProfile, ProfileError>;
  impl DeviceProfile {
      pub fn from_toml(origin: &str, text: &str) -> Result<DeviceProfile, ProfileError>;
      pub fn soc_config(&self, options: MachineOptions) -> SocConfig;
      pub fn instantiate(&self, rom: RomImage, options: MachineOptions) -> Esp32C3;
  }
  ```

La spec (§3.3) prévoit bien plus de champs : taille de flash, puces externes, câblage, échelles de boutons, tables de partitions, habillage. Chaque champ entre dans le profil avec le premier code qui le lit (flash et partitions en 1b-2, puces et câblage en SP2). Un profil qui porterait aujourd'hui des champs que rien ne valide serait de la donnée non vérifiée ; `deny_unknown_fields` garantit qu'aucune clé n'est ignorée en silence.

- [ ] **Step 1: Écrire les profils**

`crates/boards/profiles/x4.toml` :

```toml
# Xteink X4. Hardware facts and their sources: docs/research/xteink-hardware.md.
id = "x4"
name = "Xteink X4"

[soc]
family = "esp32c3"
# Assumed: the silicon revision of retail units is not established. v0.3 is the most common
# ESP32-C3 revision and selects the rev3 mask ROM.
revision = "v0.3"
```

`crates/boards/profiles/x3.toml` : identique avec `id = "x3"` et `name = "Xteink X3"`.

- [ ] **Step 2: Écrire les tests (`lib.rs`, module `tests`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use soc_esp32c3::test_support::flat_rom;

    const VALID: &str = "id = \"demo\"\nname = \"Demo board\"\n\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v1.1\"\n";

    #[test]
    fn builtin_profiles_load_and_carry_their_own_id() {
        assert_eq!(builtin_ids(), ["x3", "x4"]);
        for id in builtin_ids() {
            let profile = builtin(id).unwrap();
            assert_eq!(profile.id, *id);
            assert_eq!(profile.soc, SocFamily::Esp32C3);
            assert_eq!(profile.chip_revision, ChipRevision::V0_3);
            assert!(profile.name.starts_with("Xteink"));
        }
    }

    #[test]
    fn unknown_device_lists_the_known_ones() {
        let err = builtin("x5").unwrap_err().to_string();
        assert!(err.contains("x5") && err.contains("x3") && err.contains("x4"), "{err}");
    }

    #[test]
    fn valid_profile_parses() {
        let profile = DeviceProfile::from_toml("demo.toml", VALID).unwrap();
        assert_eq!((profile.id.as_str(), profile.name.as_str()), ("demo", "Demo board"));
        assert_eq!(profile.chip_revision.to_string(), "v1.1");
    }

    fn error_for(text: &str) -> String {
        DeviceProfile::from_toml("demo.toml", text).unwrap_err().to_string()
    }

    #[test]
    fn malformed_revision_points_at_its_line_and_says_how_to_write_it() {
        let err = error_for(&VALID.replace("v1.1", "1.1"));
        assert!(err.contains("demo.toml") && err.contains("line 6") && err.contains("v<major>.<minor>"), "{err}");
    }

    #[test]
    fn unknown_soc_family_points_at_its_line_and_lists_the_supported_ones() {
        let err = error_for(&VALID.replace("esp32c3", "esp8266"));
        assert!(err.contains("line 5") && err.contains("esp8266") && err.contains("esp32c3"), "{err}");
    }

    #[test]
    fn unknown_key_is_rejected_rather_than_ignored() {
        let err = error_for(&format!("{VALID}flash_size = \"16MB\"\n"));
        assert!(err.contains("flash_size") && err.contains("line 7"), "{err}");
    }

    #[test]
    fn missing_key_and_broken_toml_are_reported_with_the_file_name() {
        assert!(error_for("id = \"demo\"\n[soc]\nfamily = \"esp32c3\"\nrevision = \"v0.3\"\n").contains("name"));
        let broken = error_for("id = \n");
        assert!(broken.contains("demo.toml") && broken.contains("line 1"), "{broken}");
    }

    #[test]
    fn options_and_profile_combine_into_the_soc_configuration() {
        let profile = DeviceProfile::from_toml("demo.toml", VALID).unwrap();
        let config = profile.soc_config(MachineOptions { strict_mmio: true, trace_hash: true });
        assert_eq!(config.chip_revision.to_string(), "v1.1");
        assert!(config.strict_mmio && config.trace_hash);
        assert_eq!(profile.soc_config(MachineOptions::default()), SocConfig { chip_revision: config.chip_revision, ..SocConfig::default() });
    }

    #[test]
    fn instantiated_machine_sits_at_the_reset_vector() {
        let machine = builtin("x4").unwrap().instantiate(flat_rom(&[0x0000_006f]), MachineOptions::default());
        assert_eq!(machine.pc(), 0x4000_0000);
    }
}
```

- [ ] **Step 3: Lancer, constater l'échec de compilation**

Run: `cargo test -p boards > /tmp/boards.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 4: Implémenter `lib.rs`**

```rust
//! Device profiles: what a board is made of, as data.
#![cfg_attr(test, allow(clippy::unwrap_used))]
use serde::Deserialize;
use soc_esp32c3::{ChipRevision, Esp32C3, RomImage, SocConfig};

const BUILTIN: [(&str, &str); 2] = [("x3", include_str!("../profiles/x3.toml")), ("x4", include_str!("../profiles/x4.toml"))];
const BUILTIN_IDS: [&str; 2] = ["x3", "x4"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SocFamily {
    Esp32C3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub id: String,
    pub name: String,
    pub soc: SocFamily,
    pub chip_revision: ChipRevision,
}

/// Run-time choices that are not properties of the device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MachineOptions {
    pub strict_mmio: bool,
    pub trace_hash: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("{origin}: {detail}; fix that line of the device profile")]
    Parse { origin: String, detail: String },
    #[error("no device named `{id}`; choose one of: {known}")]
    UnknownDevice { id: String, known: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    id: String,
    name: String,
    soc: RawSoc,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSoc {
    family: SocFamily,
    #[serde(deserialize_with = "chip_revision")]
    revision: ChipRevision,
}

/// Parsing inside the deserializer lets the TOML error carry the line of the offending value.
fn chip_revision<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<ChipRevision, D::Error> {
    String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
}

#[must_use]
pub fn builtin_ids() -> &'static [&'static str] {
    &BUILTIN_IDS
}

/// # Errors
/// [`ProfileError::UnknownDevice`] when no built-in profile has this id.
pub fn builtin(id: &str) -> Result<DeviceProfile, ProfileError> {
    let (_, text) = BUILTIN
        .iter()
        .find(|(known, _)| *known == id)
        .ok_or_else(|| ProfileError::UnknownDevice { id: id.to_owned(), known: BUILTIN_IDS.join(", ") })?;
    DeviceProfile::from_toml(&format!("built-in profile {id}"), text)
}

impl DeviceProfile {
    /// # Errors
    /// [`ProfileError::Parse`] with the line of the first problem.
    pub fn from_toml(origin: &str, text: &str) -> Result<DeviceProfile, ProfileError> {
        let raw: RawProfile = toml::from_str(text).map_err(|e| ProfileError::Parse { origin: origin.to_owned(), detail: e.to_string().trim_end().to_owned() })?;
        Ok(DeviceProfile { id: raw.id, name: raw.name, soc: raw.soc.family, chip_revision: raw.soc.revision })
    }

    #[must_use]
    pub fn soc_config(&self, options: MachineOptions) -> SocConfig {
        SocConfig { chip_revision: self.chip_revision, strict_mmio: options.strict_mmio, trace_hash: options.trace_hash, ..SocConfig::default() }
    }

    /// Builds the device in its power-on reset state.
    #[must_use]
    pub fn instantiate(&self, rom: RomImage, options: MachineOptions) -> Esp32C3 {
        match self.soc {
            SocFamily::Esp32C3 => Esp32C3::new(rom, &self.soc_config(options)),
        }
    }
}
```

`Cargo.toml` de `boards` : dépendances `soc-esp32c3`, `serde`, `toml`, `thiserror` ; dev-dépendance `soc-esp32c3` avec `test-support`. Si le message de `toml` ne contient pas `line N` pour une erreur levée par `deserialize_with` ou `deny_unknown_fields`, ne pas affaiblir les tests : récupérer la position par `toml::de::Error::span()` et calculer le numéro de ligne dans `from_toml`.

- [ ] **Step 5: Lancer les tests**

Run: `cargo test -p boards > /tmp/boards.log 2>&1; echo "exit=$?"; cargo clippy -p boards --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"; cargo deny check > /tmp/deny.log 2>&1; echo "deny=$?"`
Expected: `exit=0`, `clippy=0`, `deny=0`.

- [ ] **Step 6: Commit**

```bash
/usr/bin/git add crates/boards Cargo.toml Cargo.lock
/usr/bin/git commit -m "feat(boards): add device profiles for the Xteink X4 and X3" -- crates/boards Cargo.toml Cargo.lock
```

---

### Task 11: `inkemu run` et `inkemu rom fetch`

**Files:**
- Create: `crates/emu-cli/src/run.rs`, `crates/emu-cli/tests/run_cli.rs`
- Modify: `crates/emu-cli/src/main.rs`, `crates/emu-cli/src/lib.rs`, `crates/emu-cli/Cargo.toml` (dépendance `boards`)

**Interfaces:**
- Consumes: `boards::{builtin, builtin_ids, DeviceProfile, MachineOptions}`, `rom_store::*` (Task 9), `soc_esp32c3::{Esp32C3, ConsoleChannel, StopReason, Diagnostic}`.
- Produces:
  ```rust
  // emu_cli::run
  pub struct RunOptions { pub max_cycles: Option<u64>, pub console: ConsoleChannel }
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum RunEnd { Budget, ConsoleClosed, ConsoleError, StrictMmio, Idle }
  impl RunEnd { pub const fn exit_code(self) -> u8 }   // Budget 0, ConsoleClosed 0, ConsoleError 2, StrictMmio 3, Idle 4
  pub fn execute(machine: &mut Esp32C3, options: &RunOptions, console: &mut dyn Write, log: &mut dyn Write) -> RunEnd;
  ```
  Ligne de commande :
  ```
  inkemu run --device <id> [--max-cycles <N>] [--strict-mmio] [--console uart0|usb-serial-jtag] [--rom-dir <DIR>] [--trace-hash]
  inkemu rom fetch (--device <id> | --all) [--rom-dir <DIR>]
  ```
  Sortie standard : **uniquement** les octets de la console du firmware. Sortie d'erreur : en-tête, avertissements (`warning: …`), ligne de fin. Codes de sortie : 0 (limite de cycles atteinte, ou console fermée par le lecteur), 2 (erreur d'entrée : appareil inconnu, ROM indisponible), 3 (accès non émulé en mode strict), 4 (le CPU attend sans réveil possible).

`--rom-dir` sur `run` : utiliser ce répertoire et ne jamais télécharger. Sans `--rom-dir`, `run` utilise le cache par défaut et télécharge la ROM au premier lancement. `rom fetch` télécharge dans le cache par défaut, ou dans `--rom-dir` s'il est donné.

- [ ] **Step 1: Écrire les tests de `execute` (`run.rs`, module `tests`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use boards::MachineOptions;
    use cpu_rv32::asm::{addi, lui, lw, sw, wfi};
    use soc_esp32c3::test_support::flat_rom;
    use std::io;

    const HANG: u32 = 0x0000_006f;
    const UART0: u32 = 0x6000_0000;
    const USB: u32 = 0x6004_3000;

    fn machine(program: &[u32], options: MachineOptions) -> Esp32C3 {
        boards::builtin("x4").unwrap().instantiate(flat_rom(program), options)
    }
    fn printing(base: u32, text: &[u8], tail: &[u32]) -> Vec<u32> {
        let mut program = vec![lui(5, base)];
        for &byte in text {
            program.extend([addi(6, 0, i32::from(byte)), sw(6, 5, 0)]);
        }
        program.extend_from_slice(tail);
        program
    }
    fn options(max_cycles: u64) -> RunOptions {
        RunOptions { max_cycles: Some(max_cycles), console: ConsoleChannel::Uart0 }
    }
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn console_bytes_go_to_the_console_writer_and_nothing_else_does() {
        let mut m = machine(&printing(UART0, b"hello\n", &[HANG]), MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        assert_eq!(execute(&mut m, &options(10_000), &mut console, &mut log), RunEnd::Budget);
        assert_eq!(console, b"hello\n");
        assert!(!text(&log).contains("hello"));
    }

    #[test]
    fn max_cycles_ends_the_run_cleanly() {
        let mut m = machine(&[HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &options(2_500_000), &mut console, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::Budget, 0));
        assert!(m.cycles().get() >= 2_500_000 && m.cycles().get() < 2_500_100, "the limit is honoured across slices");
        let log = text(&log);
        assert!(log.contains("cycle limit") && log.contains("--max-cycles") && log.contains("0x40000000"), "{log}");
    }

    #[test]
    fn selected_console_channel_is_the_one_printed() {
        let program = printing(USB, b"usb", &[addi(6, 0, 1), sw(6, 5, 4), HANG]);
        let mut m = machine(&program, MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        execute(&mut m, &RunOptions { max_cycles: Some(10_000), console: ConsoleChannel::UsbSerialJtag }, &mut console, &mut log);
        assert_eq!(console, b"usb");
    }

    #[test]
    fn unmodeled_access_is_a_named_warning_and_the_run_goes_on() {
        let ledc = svd_regs::esp32c3::registers().iter().find(|r| r.peripheral == "LEDC").unwrap();
        let mut m = machine(&[lui(5, ledc.addr), lw(6, 5, 0), HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        assert_eq!(execute(&mut m, &options(10_000), &mut console, &mut log), RunEnd::Budget);
        let log = text(&log);
        assert_eq!(log.matches("warning: ").count(), 1, "{log}");
        assert!(log.contains("LEDC") && log.contains("0x40000004"), "{log}");
    }

    #[test]
    fn strict_mode_ends_with_its_own_exit_code_and_names_the_register() {
        let ledc = svd_regs::esp32c3::registers().iter().find(|r| r.peripheral == "LEDC").unwrap();
        let mut m = machine(&[lui(5, ledc.addr), lw(6, 5, 0), HANG], MachineOptions { strict_mmio: true, trace_hash: false });
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &options(10_000), &mut console, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::StrictMmio, 3));
        let log = text(&log);
        assert!(log.contains("--strict-mmio") && log.contains("LEDC"), "{log}");
    }

    #[test]
    fn idle_cpu_ends_the_run_and_says_why() {
        let mut m = machine(&[wfi(), HANG], MachineOptions::default());
        let (mut console, mut log) = (Vec::new(), Vec::new());
        let end = execute(&mut m, &RunOptions { max_cycles: None, console: ConsoleChannel::Uart0 }, &mut console, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::Idle, 4));
        assert!(text(&log).contains("waiting for an interrupt"), "{}", text(&log));
    }

    #[test]
    fn trace_hash_is_printed_when_enabled() {
        let run = || {
            let mut m = machine(&printing(UART0, b"x", &[HANG]), MachineOptions { strict_mmio: false, trace_hash: true });
            let (mut console, mut log) = (Vec::new(), Vec::new());
            execute(&mut m, &options(5_000), &mut console, &mut log);
            text(&log).lines().find(|l| l.starts_with("trace-hash ")).map(str::to_owned)
        };
        let line = run().expect("a trace-hash line");
        assert_eq!(line.len(), "trace-hash ".len() + 16);
        assert_eq!(Some(line), run());
    }

    /// A console whose reader went away, like `inkemu run | head -1`.
    struct Closed;
    impl io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn closed_stdout_does_not_panic() {
        let mut m = machine(&printing(UART0, b"hello\n", &[HANG]), MachineOptions::default());
        let mut log = Vec::new();
        let end = execute(&mut m, &RunOptions { max_cycles: None, console: ConsoleChannel::Uart0 }, &mut Closed, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleClosed, 0));
    }
}
```

- [ ] **Step 2: Écrire les tests de la ligne de commande (`crates/emu-cli/tests/run_cli.rs`)**

```rust
use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn inkemu() -> Command {
    Command::cargo_bin("inkemu").unwrap()
}

#[test]
fn unknown_device_exits_2_and_lists_the_known_ones() {
    inkemu().args(["run", "--device", "x9"]).assert().code(2).stderr(contains("x9").and(contains("x3")).and(contains("x4")));
}

#[test]
fn empty_rom_dir_exits_2_naming_the_file_and_never_downloads() {
    let dir = tempfile::tempdir().unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stderr(contains("esp32c3_rev3_rom.elf").and(contains("esp-rom-elfs")));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn tampered_rom_is_refused_before_anything_runs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("esp32c3_rev3_rom.elf"), b"not the ROM").unwrap();
    inkemu()
        .args(["run", "--device", "x4", "--max-cycles", "1000", "--rom-dir"])
        .arg(dir.path())
        .assert()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(contains("expected sha256 19ac22e0").and(contains("got ")));
}

#[test]
fn rom_fetch_requires_a_device_or_all() {
    inkemu().args(["rom", "fetch"]).assert().code(2);
}

/// The real ROM, through the real command. `INKEMU_ROM_ELF_DIR` must hold the three ROM ELF files.
#[test]
#[ignore = "needs INKEMU_ROM_ELF_DIR"]
fn x4_boots_the_real_rom_to_its_banner() {
    let dir = std::env::var("INKEMU_ROM_ELF_DIR").expect("set INKEMU_ROM_ELF_DIR");
    let run = || {
        inkemu()
            .args(["run", "--device", "x4", "--max-cycles", "5000000", "--trace-hash", "--rom-dir", &dir])
            .assert()
            .code(0)
            .get_output()
            .clone()
    };
    let first = run();
    let console = String::from_utf8_lossy(&first.stdout).replace('\r', "");
    assert!(
        console.starts_with("ESP-ROM:esp32c3-api1-20210207\nBuild:Feb  7 2021\nrst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)\n"),
        "{console}"
    );
    let log = String::from_utf8_lossy(&first.stderr).into_owned();
    assert!(log.contains("Xteink X4") && log.contains("v0.3") && log.contains("not calibrated"), "{log}");
    assert_eq!(first.stdout, run().stdout, "two runs print the same console");
    let hash = |out: &std::process::Output| String::from_utf8_lossy(&out.stderr).lines().find(|l| l.starts_with("trace-hash ")).map(str::to_owned);
    assert!(hash(&first).is_some());
    assert_eq!(hash(&first), hash(&run()));
}
```

Ajouter dans `crates/emu-cli/tests/inspect_cli.rs` (point reporté de SP1a : `inspect` paniquait sur un `stdout` fermé) :

```rust
#[test]
fn inspect_with_a_closed_stdout_does_not_panic() {
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.bin");
    std::fs::write(&path, firmware::builder::build_image(&firmware::builder::ImageSpec::default())).unwrap();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin("inkemu"))
        .arg("inspect")
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take()); // the reader goes away before inkemu writes
    let output = child.wait_with_output().unwrap();
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"), "{output:?}");
    assert!(output.status.success(), "{output:?}");
}
```

Adapter la construction de l'image à l'API réelle du constructeur de test de `firmware` (voir les tests existants de ce fichier).

- [ ] **Step 3: Lancer, constater l'échec de compilation**

Run: `cargo test -p emu-cli > /tmp/cli.log 2>&1; echo "exit=$?"`
Expected: `exit=101`.

- [ ] **Step 4: Implémenter `run.rs`**

```rust
//! `inkemu run`: drives a machine and relays its console and diagnostics.
use emu_core::Cycles;
use soc_esp32c3::{ConsoleChannel, Esp32C3, StopReason};
use std::io::{self, Write};

/// Cycles executed between two flushes of the console and the diagnostics: a fraction of a second
/// of host time, so output appears as the firmware prints it.
const SLICE_CYCLES: u64 = 1_000_000;

#[derive(Clone, Copy, Debug)]
pub struct RunOptions {
    /// Stop when the CPU cycle counter reaches this value; `None` runs until the machine stops by itself.
    pub max_cycles: Option<u64>,
    pub console: ConsoleChannel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    Budget,
    /// Whoever was reading the console went away (`inkemu run | head`): a normal way to end.
    ConsoleClosed,
    /// The console could not be written for any other reason.
    ConsoleError,
    StrictMmio,
    Idle,
}

impl RunEnd {
    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            RunEnd::Budget | RunEnd::ConsoleClosed => 0,
            RunEnd::ConsoleError => 2,
            RunEnd::StrictMmio => 3,
            RunEnd::Idle => 4,
        }
    }
}

/// Writes console bytes; `Ok(false)` when the reader is gone.
fn relay(console: &mut dyn Write, bytes: &[u8]) -> io::Result<bool> {
    if bytes.is_empty() {
        return Ok(true);
    }
    match console.write_all(bytes).and_then(|()| console.flush()) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn execute(machine: &mut Esp32C3, options: &RunOptions, console: &mut dyn Write, log: &mut dyn Write) -> RunEnd {
    let end = loop {
        let now = machine.cycles().get();
        let slice_end = now.saturating_add(SLICE_CYCLES);
        let limit = options.max_cycles.map_or(slice_end, |max| max.min(slice_end));
        let stop = machine.run_until(Cycles(limit));
        let console_open = relay(console, &machine.take_console(options.console));
        for diagnostic in machine.drain_diagnostics() {
            // A failing log writer must not hide the run's outcome: there is nowhere left to report it.
            let _ = writeln!(log, "warning: {diagnostic}");
        }
        match (console_open, stop) {
            (Ok(false), _) => break RunEnd::ConsoleClosed,
            (Err(e), _) => {
                let _ = writeln!(log, "error: cannot write the console output: {e}; check where stdout is redirected");
                break RunEnd::ConsoleError;
            }
            (Ok(true), StopReason::StrictMmio(diagnostic)) => {
                let _ = writeln!(log, "inkemu: stopped by --strict-mmio: {diagnostic}");
                break RunEnd::StrictMmio;
            }
            (Ok(true), StopReason::Idle) => {
                let _ = writeln!(log, "inkemu: the CPU is waiting for an interrupt and nothing can raise one; interrupt sources are not emulated yet");
                break RunEnd::Idle;
            }
            (Ok(true), StopReason::CycleBudget) if options.max_cycles.is_some_and(|max| machine.cycles().get() >= max) => {
                let _ = writeln!(log, "inkemu: cycle limit reached; raise --max-cycles to run longer");
                break RunEnd::Budget;
            }
            (Ok(true), StopReason::CycleBudget) => {}
        }
    };
    let _ = writeln!(log, "inkemu: stopped at pc {:#010x} after {} cycles ({} instructions)", machine.pc(), machine.cycles().get(), machine.instructions());
    if let Some(hash) = machine.trace_hash() {
        let _ = writeln!(log, "trace-hash {hash:016x}");
    }
    end
}
```

Ajouter au module `tests` le cas d'une erreur d'écriture qui n'est pas une fermeture :

```rust
    struct Failing;
    impl io::Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn console_write_error_is_reported_and_ends_the_run_with_an_error_code() {
        let mut m = machine(&printing(UART0, b"hello\n", &[HANG]), MachineOptions::default());
        let mut log = Vec::new();
        let end = execute(&mut m, &RunOptions { max_cycles: None, console: ConsoleChannel::Uart0 }, &mut Failing, &mut log);
        assert_eq!((end, end.exit_code()), (RunEnd::ConsoleError, 2));
        assert!(text(&log).contains("disk full"), "{}", text(&log));
    }
```

- [ ] **Step 5: Brancher les sous-commandes dans `main.rs`**

```rust
    /// Boot a device from its mask ROM and print its console.
    Run {
        /// Device profile: x4 or x3.
        #[arg(long)]
        device: String,
        /// Stop when the CPU has executed this many clock cycles.
        #[arg(long)]
        max_cycles: Option<u64>,
        /// Stop at the first access to a register the emulator does not model (exit code 3).
        #[arg(long)]
        strict_mmio: bool,
        /// Which console of the device goes to standard output.
        #[arg(long, value_enum, default_value_t = ConsoleArg::Uart0)]
        console: ConsoleArg,
        /// Use the ROM ELF files of this directory and never download.
        #[arg(long)]
        rom_dir: Option<PathBuf>,
        /// Print the execution trace hash at the end (determinism checks).
        #[arg(long)]
        trace_hash: bool,
    },
    /// Manage the mask-ROM files the emulator needs.
    Rom {
        #[command(subcommand)]
        action: RomAction,
    },
```

```rust
#[derive(Clone, Copy, clap::ValueEnum)]
enum ConsoleArg {
    Uart0,
    UsbSerialJtag,
}

#[derive(Subcommand)]
enum RomAction {
    /// Download and verify ROM files into the cache (or into --rom-dir).
    Fetch {
        /// Fetch the ROM this device needs.
        #[arg(long, conflicts_with = "all", required_unless_present = "all")]
        device: Option<String>,
        /// Fetch every ROM revision of the supported chips.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        rom_dir: Option<PathBuf>,
    },
}
```

`run` :

1. `boards::builtin(&device)` → erreur : `error: {e}`, code 2.
2. `entry = rom_store::rom_entry_for(profile.chip_revision)` ; source = `RomSource::Directory(dir)` si `--rom-dir`, sinon `RomSource::Cache { dir: default_cache_dir()?, fetch: &HttpFetch }`. Si le fichier n'est pas encore dans le cache, écrire d'abord sur la sortie d'erreur : `inkemu: downloading the ESP32-C3 mask ROM (esp-rom-elfs <release>, 4.9 MB) into <dir>`.
3. `rom_store::load_rom(&source, &ESP_ROM_ELFS, entry)` → erreur : `error: {e}`, code 2, rien sur la sortie standard.
4. En-tête sur la sortie d'erreur : `inkemu: {name} (ESP32-C3 {revision}, mask ROM rev{rom_revision}); timing model {timing_name} (not calibrated)`. Le nom du modèle vient de `TimingModel::name()` et la mention « not calibrated » de `is_calibrated()` ; exposer pour cela `Esp32C3::timing_model(&self) -> &dyn emu_core::TimingModel` dans `soc-esp32c3` (une ligne, avec un test qui vérifie `!is_calibrated()`).
5. `profile.instantiate(rom, MachineOptions { strict_mmio, trace_hash })`, puis `run::execute(&mut machine, &options, &mut io::stdout().lock(), &mut io::stderr().lock())` et `ExitCode::from(end.exit_code())`.

`rom fetch` : pour chaque entrée voulue (celle de l'appareil, ou les trois de `ESP32C3_ROMS` avec `--all`), `load_rom` avec `RomSource::Cache { dir: rom_dir ou cache par défaut, fetch: &HttpFetch }`, puis une ligne sur la sortie standard : le chemin du fichier ELF vérifié. Erreur : `error: {e}`, code 2.

`inspect` : remplacer `print!` par une écriture explicite sur `io::stdout().lock()` ; une erreur `BrokenPipe` termine en succès, toute autre erreur d'écriture donne `error: cannot write to standard output: {e}; check where stdout is redirected` et le code 2.

- [ ] **Step 6: Lancer toute la suite, puis la vraie ROM**

Run: `cargo test --workspace > /tmp/ws.log 2>&1; echo "exit=$?"; cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"; cargo fmt --all --check > /tmp/fmt.log 2>&1; echo "fmt=$?"`
Expected: `exit=0`, `clippy=0`, `fmt=0`.

Run: `INKEMU_ROM_ELF_DIR=<répertoire des ELF> cargo test -p emu-cli --test run_cli -- --ignored > /tmp/real-cli.log 2>&1; echo "exit=$?"`
Expected: `exit=0`.

Run: `cargo run --release -p emu-cli -- run --device x4 --max-cycles 5000000 --rom-dir <répertoire des ELF> > /tmp/console.txt 2> /tmp/run.log; echo "exit=$?"; cat /tmp/console.txt; tail -5 /tmp/run.log`
Expected: `exit=0`, la bannière de trois lignes dans `console.txt`, puis dans `run.log` des avertissements nommés (registres SPI, GPIO…) et la ligne de fin.

- [ ] **Step 7: Commit**

```bash
/usr/bin/git add crates/emu-cli crates/soc-esp32c3 Cargo.toml Cargo.lock
/usr/bin/git commit -m "feat(cli): add inkemu run and inkemu rom fetch" -- crates/emu-cli crates/soc-esp32c3 Cargo.toml Cargo.lock
```

---

### Task 12: CI sur la vraie ROM et documentation

**Files:**
- Modify: `.github/workflows/ci.yml`, `README.md`, `docs/adr/0001-espressif-rom-and-svd-licensing.md`, `docs/superpowers/notes/2026-10-11-sp1a-carryover.md`
- Create: `NOTICE`

**Interfaces:**
- Consumes: `inkemu rom fetch --all --rom-dir` (Task 11), les tests ignorés `real_rom` et `run_cli`.
- Produces: un job CI `rom-boot` qui démarre la vraie ROM à chaque push.

- [ ] **Step 1: Ajouter le job `rom-boot`**

Dans `.github/workflows/ci.yml` :

```yaml
  rom-boot:
    # Boots the real ESP32-C3 mask ROM. The ROM is downloaded from Espressif's pinned release and
    # verified by inkemu itself; it is never stored in this repository.
    runs-on: ubuntu-24.04
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with: { toolchain: "1.99.0" }
      - uses: Swatinem/rust-cache@v2
      - uses: actions/cache@v4
        with:
          path: ${{ runner.temp }}/rom
          key: esp-rom-elfs-20260528
      - name: Fetch and verify the mask ROMs
        run: cargo run --release -p emu-cli -- rom fetch --all --rom-dir "$RUNNER_TEMP/rom"
      - name: Boot the ROM
        env:
          INKEMU_ROM_ELF_DIR: ${{ runner.temp }}/rom
        run: |
          cargo test --release -p soc-esp32c3 --test real_rom -- --ignored
          cargo test --release -p emu-cli --test run_cli -- --ignored
```

Le cache d'Actions ne remplace aucune vérification : `rom fetch` recontrôle l'empreinte de chaque fichier présent.

- [ ] **Step 2: Mettre à jour le README**

Remplacer le paragraphe d'état, la section « Build & try » et la ligne 1 de la feuille de route. Texte anglais :

````markdown
> **Status: early development.** inkemu boots the real ESP32-C3 mask ROM from its reset vector and prints its banner. The RV32IMC core is validated against riscv-tests and, through RISCOF, against the Sail reference model. Flash, the second-stage bootloader and the application come next (sub-projects 1b-2 to 1b-4). See the [design spec](docs/superpowers/specs/2026-10-11-inkemu-design.md) and the [roadmap](#roadmap).

## Build & try

```bash
cargo build --release

# Boot a virtual Xteink X4 from its mask ROM and print its console.
target/release/inkemu run --device x4 --max-cycles 5000000
```

```text
ESP-ROM:esp32c3-api1-20210207
Build:Feb  7 2021
rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)
```

The device console goes to standard output. Everything inkemu has to say goes to standard error: the device and timing model in use, then one `warning:` line for each register the firmware touches that inkemu does not model yet, with the peripheral, the register, its address and the program counter. `--strict-mmio` turns the first of them into a stop, for CI.

The first run downloads the ROM (4.9 MB) from Espressif's `esp-rom-elfs` release `20260528` and checks its SHA-256 before using it. To work offline, extract the archive yourself and pass `--rom-dir <directory>`; `inkemu rom fetch --all` fills a directory in advance.

Other commands: `inkemu inspect <app.bin>` describes an ESP-IDF application image and explains what is wrong when it refuses a file; `inkemu rvtest <elf>` runs a bare-metal RISC-V conformance test.
````

Ajouter, avant « License », une section :

```markdown
## Third-party material

- **Mask ROM.** The ESP32-C3 ROM is Espressif's code and is not open source. inkemu never ships it: it downloads the ROM ELF files that Espressif publishes under Apache-2.0 in [`esp-rom-elfs`](https://github.com/espressif/esp-rom-elfs) and verifies them against pinned hashes.
- **Register descriptions.** `third_party/espressif-svd/` holds an unmodified copy of Espressif's `esp32c3.svd` (Apache-2.0), from which `crates/svd-regs` is generated.

See [ADR 0001](docs/adr/0001-espressif-rom-and-svd-licensing.md).
```

Mettre à jour le paragraphe « État » de la section française dans le même sens, et la ligne 1 de la feuille de route : « *1a done; 1b-1, the SoC skeleton and the ROM boot, done; 1b-2, flash and the bootloader, is next.* »

- [ ] **Step 3: Créer `NOTICE` à la racine**

```text
inkemu
Copyright 2026 Rony Licha

This product includes register descriptions derived from Espressif's CMSIS-SVD files
(https://github.com/espressif/svd), Copyright Espressif Systems (Shanghai) Co., Ltd.,
licensed under the Apache License, Version 2.0. See third_party/espressif-svd/.
```

Vérifier le nom du titulaire dans `LICENSE` ou dans l'en-tête du README avant d'écrire la ligne de copyright ; s'il n'y figure pas, omettre cette ligne plutôt que de l'inventer.

- [ ] **Step 4: Amender l'ADR 0001**

Ajouter à la fin une section « Mise en œuvre (SP1b-1, 2026-10-11) » qui consigne, sans réécrire la décision :

- les trois points « inconnus » levés : noms des fichiers dans l'archive, contenu complet de la ROM dans les ELF (avec la recette en cinq étapes et sa validation octet pour octet pour rev3), empreintes par fichier — renvoyer au tableau du manifeste dans `crates/emu-cli/src/rom_store.rs` et à `docs/research/esp32c3-rom-elfs.md`, §9 ;
- le cache ne contient que l'ELF vérifié ; l'image est reconvertie à chaque lancement et son empreinte vérifiée (écart assumé par rapport à « convertit en images » dans « Conséquences ») ;
- un seul SVD est copié pour l'instant (`esp32c3.svd`) ; celui du S3 le sera avec le sous-projet 7 ;
- l'image `esp32c3-rom.bin` de QEMU a servi une fois de référence de comparaison, dans un conteneur, sans être copiée ni distribuée.

- [ ] **Step 5: Marquer les points de SP1a traités**

Dans `docs/superpowers/notes/2026-10-11-sp1a-carryover.md`, barrer (`~~…~~ **Levé par SP1b-1**`) : `Bus::page_version` et l'alias IRAM/DRAM, l'écriture de `mcycle`, le `debug_assert` de `set_interrupt_pending`, la panique d'`inspect` sur un `stdout` fermé, et le dernier point (« Lancer `inkemu inspect` sur un vrai firmware CrossPoint » : fait le 2026-10-11 avec l'accord de l'utilisateur).

- [ ] **Step 6: Vérification finale**

Run: `cargo fmt --all --check > /tmp/fmt.log 2>&1; echo "fmt=$?"; cargo clippy --workspace --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy=$?"; cargo test --workspace > /tmp/ws.log 2>&1; echo "test=$?"; cargo run -p svd-gen -- --check > /tmp/svd.log 2>&1; echo "svd=$?"; cargo deny check > /tmp/deny.log 2>&1; echo "deny=$?"; cargo llvm-cov --workspace --fail-under-lines 80 > /tmp/cov.log 2>&1; echo "cov=$?"`
Expected: les six codes à 0.

- [ ] **Step 7: Commit**

```bash
/usr/bin/git add .github/workflows/ci.yml README.md NOTICE docs/adr/0001-espressif-rom-and-svd-licensing.md docs/superpowers/notes/2026-10-11-sp1a-carryover.md
/usr/bin/git commit -m "docs: document the ROM boot milestone and boot the real ROM in CI" -- .github/workflows/ci.yml README.md NOTICE docs/adr/0001-espressif-rom-and-svd-licensing.md docs/superpowers/notes/2026-10-11-sp1a-carryover.md
```

---

## Critères d'acceptation de ce plan

1. `inkemu run --device x4 --max-cycles 5000000` affiche sur la sortie standard, avec la vraie ROM rev3, les trois lignes `ESP-ROM:esp32c3-api1-20210207`, `Build:Feb  7 2021`, `rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)`, et sort en 0.
2. En mode `--strict-mmio`, la bannière est complète avant l'arrêt : tout ce que la ROM touche jusque-là est modélisé, sans exception CPU.
3. Les ROM rev0 et rev101 affichent leur propre bannière.
4. Deux exécutions donnent la même console, le même nombre de cycles et la même empreinte de trace.
5. L'image de ROM rev3 reconstruite a l'empreinte `0de1e65020e803bea0d7443dca149d61895e01fca3bb9c82d073234eebd73f99`.
6. Les accès aux registres non modélisés après la bannière apparaissent comme avertissements nommés, un par registre et par sens.
7. `cargo test --workspace`, clippy, fmt, `svd-gen --check`, `cargo deny check` et la couverture (≥ 80 %) passent ; le job CI `rom-boot` est vert.
