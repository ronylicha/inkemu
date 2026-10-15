# SP2e — Note de passation

Branche de SP2e, partie de `main` à `4b02a91`. Rien n'est poussé. Spec :
`docs/superpowers/specs/2026-10-15-sp2e-sd-card-design.md` ; plan : `docs/superpowers/plans/2026-10-15-sp2e-sd-card.md`
(règles 2e-R1 à 2e-R9) ; recherche : `docs/research/sp2e-sd-card.md` ; ADR 0014.

## Résultat

| Commit | Contenu |
|---|---|
| `df7dbd3` | recherche (PLS 9.10, SdFat 2.3.1, FreeInk `111fdcc7`, CrossPoint 1.6.5), spec, plan, ADR 0014 |
| `c3b7dec` | `chips::sdcard` : la carte SD en SPI au niveau des octets, le stockage par blocs, les registres, les CRC, les diagnostics ; 67 tests unitaires |
| `1240f37` | `chips/tests/sd_driver.rs` : un pilote écrit d'après ce que fait SdFat 2.3.1 initialise, lit et écrit une SDSC et une SDHC ; logement vide, carte trop lente, carte protégée |
| `18a5aa5` | `host::sd_image` : fichier image verrouillé, lu bloc à bloc, écrit en place par un journal ; 12 tests |
| `7a8baf4` | `host::sd_format` : image vierge FAT32 creuse et déterministe ; relue par `fatfs`, montée à travers la carte (`host/tests/sd_card.rs`) ; verrou Cargo de l'enveloppe Tauri mis à jour |
| `12b4079` | `inkemu sd create` ; README EN/FR |
| `00303b1` | relecture de la PR #19, écart de protocole : occupation après le jeton d'arrêt `FDh` et après CMD12 qui termine une écriture multiple (R1b), aucune après CMD12 qui termine une lecture (`SdTiming::stop_ns`, 250 µs) |
| `f3c64c2` | relecture de la PR #19, fichier image : journal non appliqué fini avant la vie suivante ou vie refusée ; journal lié à son image (`inkemu-sd-journal 2`) ; longueur bornée avant lecture, lecture en flux ; trou percé à l'effacement (Linux), zéros sur les seules données ailleurs ; changements vérifiés avant le journal ; type revérifié sur le descripteur ; image vierge mise en place sans jamais remplacer un fichier, chemins revendiqués et journal laissé refusés |

Chaque constat de la relecture a son test qui échoue sans la correction ; pour l'application inachevée, l'identité,
le type revérifié, la vérification des changements et le trou percé, cela a été constaté en neutralisant la
correction puis en la rétablissant.

Vérifié en local (`CARGO_BUILD_JOBS=3`) : `cargo fmt --all --check` ; `cargo clippy --all-targets -- -D warnings` sur
`chips`, `host`, `emu-cli` ; `cargo test -p chips -p host` (159 + 6 + 10, 296 + 2 + 3, tout passe) ; `cargo test -p
emu-cli --test sd_cli` ; `cargo check -p chips --target wasm32-unknown-unknown` ; `cargo doc --no-deps` de `chips` et
`host` avec `-D warnings` ; `cargo deny check` (licences, interdictions, sources, avis : `fatfs` 0.3.6 MIT, `bitflags`
1.3.2 et `byteorder` 1.5.0 entrent en dépendances de test de `host`, sans doublon nouveau). Laissé à la CI : la suite
complète, la couverture, l'enveloppe Tauri (`cargo clippy --locked` dans `app/src-tauri`, dont le verrou suit la
nouvelle dépendance `host → chips`).

## Interface du modèle (pour SP2d)

```rust
// chips::sdcard
pub const BLOCK_BYTES: usize = 512;            pub type Block = [u8; 512];
pub trait BlockSource: Debug { fn block_count(&self) -> u64;
    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError>; }
pub trait BlockStorage: BlockSource { fn write_block(&mut self, lba: u64, data: &Block) -> Result<(), StorageError>;
    fn erase(&mut self, first: u64, count: u64) -> Result<(), StorageError>; }
MemoryStorage::{new(blocks: usize), from_bytes(Vec<u8>) -> Result, bytes(), into_bytes()}
Overlay<S: BlockSource>::{new(base), changes() -> Changes<'_>, base(), into_base()}  // BlockStorage
Changes::{erased() -> impl Iterator<(u64 first, u64 count)>, blocks() -> impl Iterator<(u64, &Block)>,
          written_count(), is_empty()}
SparseBlocks::{new(block_count: u64), insert(lba, Block) -> Result, inserted()}        // BlockSource
Capacity::{new(bytes) -> Result<Capacity, CapacityError>, bytes(), blocks(), kind(), csd(write_protected), told_by(&csd)}
CardKind::{Sdsc, Sdhc}, Cid { manufacturer, oem, product, revision, serial, year, month } (Default)
SdTiming { initialization_ns, read_access_ns, program_ns, erase_ns }  // défauts 50 ms, 250 µs, 1 ms, 2 ms
SdCardConfig { cid: Cid, write_protected: bool, timing: SdTiming }   // Default
SdCard<S: BlockStorage>::new(config, storage: S, time_base: Frequency) -> Result<_, CapacityError>
    select(now: Cycles, selected: bool)
    exchange(now: Cycles, mosi: u8) -> Option<u8>        // None : MISO non piloté
    transfer(now: Cycles, mosi: &[u8], miso: &mut [u8])  // écrase ce que la carte pilote
    capacity(), config(), storage(), storage_mut(), into_storage(), is_selected(), is_in_spi_mode(),
    is_busy(now), take_diagnostics() -> Vec<SdDiagnostic>, dropped_diagnostics()
SdSlot<S>::{empty(), holding(card), card(), card_mut(), take(), insert(card), select, exchange, transfer}
SdDiagnostic { at: Cycles, kind: SdDiagnosticKind }   // 23 genres, textes distincts
```

Pas de `Clocked` : rien ne change sur les lignes de la carte sans que le maître horloge. Câblée par SP2d, la base de
temps vaut 1 GHz, comme le SSD1677 (2d-D2) : `Nanos(n)` est `Cycles(n)`.

## Interface du fichier image (pour SP2d)

```rust
// host::sd_image
pub enum Access { ReadWrite, ReadOnly }
SdImage::open(path: &Path, access: Access, claimed: &[session_file::Claimed], log: &mut dyn Write)
    -> Result<SdImage, SdImageError>      // vérifie, verrouille, reprend un journal interrompu
SdImage::{shown(), capacity(), write_protected(),
          source(&mut self) -> Result<FileBlocks, _>,   // finit d'abord une sauvegarde inachevée, ou UnfinishedSave
          commit(&mut self, &Changes<'_>) -> Result<Committed, SdImageError>}
Committed { blocks, erased_ranges, zeros_written }, Committed::notice() -> Option<String>  // à dire s'il y en a une
pub fn files_of_the_sd_image(path) -> Vec<(PathBuf, &'static str)>   // image, .<nom>.journal, .<nom>.journal.tmp
pub fn max_journal_bytes(blocks: u64) -> u64
// host::sd_format
pub fn create_blank(path, mebibytes: u64, label: &str, claimed: &[Claimed]) -> Result<Fat32Layout, SdFormatError>
Fat32Layout::{new(mebibytes, label), bytes(), regions(), describe()}, MIN_MEBIBYTES = 64, MAX_MEBIBYTES, DEFAULT_LABEL
```

Une vie : `SdCard::new(SdCardConfig { write_protected: image.write_protected(), ..Default::default() },
Overlay::new(image.source()?), 1 GHz)` ; à sa fin, `image.commit(&card.into_storage().changes())`. Un rejeu :
`Overlay::new(SparseBlocks)` reconstruit de la session, jamais le fichier. Un `commit` qui échoue après avoir écrit
son journal laisse l'image en attente : la vie suivante ne démarre avec la carte que si `source` finit ce journal ;
`device.rs` (tâche 14) dit l'erreur sinon. `create_blank` reçoit les mêmes chemins revendiqués que `SdImage::open`.

## Ce que SP2d doit intégrer, tâche par tâche

Le détail est au §9 de la spec ; en bref : **tâche 0** relire ce document et ADR 0014 ; **tâche 4** `SdLines`
(`SpiTarget` sur un `SdSlot`, base 1 GHz) ; **tâche 9** `[wiring.sd] cs = 12, miso = 7` ; **tâche 11**
`WiredOptions.sd`, attache sur CS 12 / MISO 7, poignée qui rend la surcouche ; **tâche 12** la carte dans le format 2
(spec §7 : capacité, protection, plages non nulles compressées ≤ 64 Mio, empreinte de la forme canonique ; empreinte des
changements dans le résultat ; refus avant l'exécution au-delà de la borne) ; **tâche 13** `inkemu run --sd-image
[--sd-read-only]`, chemins revendiqués dans les deux sens, ligne de résumé ; **tâche 14** `Setup.sd`, ouverture au
démarrage, commit à chaque fin de vie ; **tâche 15** la section « Carte SD » de la boîte de démarrage (spec §8) ;
**tâche 17** CrossPoint 1.6.5 avec une image vierge jusqu'à son écran d'accueil, `/.crosspoint/state.json` relu par
`fatfs` ; **tâche 18** renvois d'ADR.

## Décisions prises sans l'utilisateur

1. **Pas de crate `storage`** (la spec globale en prévoyait une) : une crate de plus touche `host_io_ban` de `boards`,
   hors périmètre. Le stockage par blocs est dans `chips`, le fichier et le formatage dans `host` (ADR 0014 point 3).
2. **Écriture en place par un journal** au lieu d'un fichier temporaire de l'image entière renommé : même garantie
   (l'image d'avant ou d'après), coût proportionnel à ce que la vie a écrit. Le verrou est sur l'image elle-même
   (exclusif ou partagé), pas sur un `.lock`, puisqu'elle n'est jamais remplacée.
3. **SDSC jusqu'à 2 Gio, SDHC/SDXC au-delà**, sans imposer le minimum SDHC de 2 Go + 8,5 Mo. Le CSD ne déclare pas la
   classe 7 (verrouillage), que la PLS fixe pour SDHC : la déclarer serait annoncer ce que le modèle ne fait pas.
4. **Réponse au deuxième octet** après une commande : SdFat jette le premier (RS §2 point 5).
5. **Durées** (hypothèses, paramètres) : initialisation 50 ms, accès en lecture 250 µs, programmation 1 ms, effacement
   2 ms ; CID d'aucun fabricant (MID 0, « IE », « INKEM »).
6. **Une désélection** abandonne une commande ou un bloc en cours d'octets et termine une lecture multiple ; l'attente
   du jeton suivant d'une écriture multiple et l'occupation se poursuivent ; CMD0 passe même pendant l'occupation.
7. **CMD0 en mode SPI garde le réglage CRC de CMD59** (PLS §7.2.2, « according to CMD59 setting »).
8. **Formatage par notre code** (FAT32 seulement, comme demandé) ; `fatfs` 0.3.6 (MIT, dernière version janvier 2023)
   en dépendance de test seulement, pour une relecture indépendante.
9. **`inkemu sd create` sort avec 2** pour toute erreur, y compris l'écriture : 4 a déjà un sens pour `run`.
10. **Interface non implémentée** : la sélection d'image n'a d'effet qu'avec la machine câblée de SP2d ; une option qui
    ne ferait rien serait une demi-mesure. Elle est spécifiée (spec §8) et confiée à la tâche 15 de SP2d.
11. **Occupation après un arrêt** (relecture) : après le jeton `FDh` (PLS figure 7-7) et après CMD12 qui termine une
    écriture multiple (R1b) ; pas après CMD12 qui termine une lecture (PLS §4.9.2 : « based on its state prior to the
    command reception »). Durée : `SdTiming::stop_ns`, 250 µs, hypothèse.
12. **Un journal non appliqué passe avant tout** (relecture) : la vie suivante le finit d'abord, ou ne démarre pas
    avec la carte. Refuser la vie plutôt que lire une image à moitié écrite ; le journal reste intact pour une reprise.
13. **Identité du fichier dans le journal** : périphérique + inode sous Unix ; ailleurs, faute d'identité stable dans la
    bibliothèque standard, les 16 premiers octets du SHA-256 du chemin canonique (un journal déplacé avec son image
    sous un autre nom serait alors refusé, ce qui est le côté sûr). Le format passe à `inkemu-sd-journal 2` ; aucun
    journal de la version 1 n'a pu être livré (rien n'est mergé).
14. **Effacement** : trou percé sous Linux ; macOS et Windows écrivent des zéros (sur les seules données là où
    `SEEK_DATA` répond), parce que `F_PUNCHHOLE` et `FSCTL_SET_ZERO_DATA` exigeraient `unsafe`, interdit dans l'espace
    de travail, et n'ont pas d'enveloppe dans `nix`/`rustix`. La fin de vie le dit (`Committed::notice`).
15. **Mise en place sans remplacement** : `renameat2(RENAME_NOREPLACE)` (Linux glibc), lien dur puis effacement
    ailleurs ; vérification puis renommage en dernier recours, sur un système de fichiers sans liens durs.

## Laissé aux héritiers

| Sujet | Héritier |
|---|---|
| Câblage, session, CLI `run`, interface (spec §9) | SP2d, tâches 4, 9, 11 à 15, 17, 18 |
| Dossier de l'hôte exposé en FAT32 virtuel (spec globale §1.1) | après la livraison du 2027-01-01 : un `BlockSource` qui synthétise le volume |
| Insérer, retirer, protéger la carte pendant une vie | SP4 (`SdSlot::take`, `SdSlot::insert` existent) |
| exFAT pour une image vierge | à la demande |
| SDMMC (X4 Pro, X4C) | SP5 |
| Verrouillage par mot de passe, groupes de protection, CPRM, CMD27, CMD56 | si un firmware en dépend (diagnostic nommé aujourd'hui) |
| Le pilote SD d'ESP-IDF (`sdspi` : CMD59, ACMD51, CMD6) sur le modèle | le firmware de référence qui l'utilisera |
| Porter le formatage dans une crate sans E/S pour la version web | SP6 |

## Limites de preuve

- **L'écran d'accueil de CrossPoint n'est pas vu** : il demande le câblage de SP2d. Ce qui est prouvé : un pilote qui
  suit SdFat 2.3.1 d'après sa source (pas SdFat lui-même) initialise, lit et écrit la carte ; une image vierge passe les
  vérifications de montage de SdFat lues à travers la carte ; une autre implémentation FAT (`fatfs`) lit le volume, crée
  `/.crosspoint/state.json` et le relit.
- Les durées de la carte sont des hypothèses (la PLS simplifiée retire ses tables de temps).
- Le pilote `sdspi` d'ESP-IDF n'a pas été joué contre le modèle ; CMD59 avec CRC, CMD6, ACMD13, ACMD51 sont testés par
  octets seulement.
- Le verrou de l'image est consultatif ; il est testé sous Linux seulement. Le caractère creux de l'image et le mode
  `0600` sont vérifiés sous Unix seulement. La synchronisation du répertoire après un renommage est au mieux.
- La reprise d'un journal est testée en simulant l'interruption (journal écrit puis processus « mort », application
  partielle), pas en tuant un processus. L'échec d'application est simulé par un point d'injection propre aux tests
  (`fail_after`, toujours `None` hors d'eux), pas par un vrai disque plein.
- Le percement de trou et le repli `SEEK_DATA` sont testés sous Linux (ext4) ; les branches macOS, FreeBSD et Windows
  (zéros) ne sont compilées et jouées que par la CI des plateformes concernées, quand elle existera.
- L'identité par inode suppose que l'image n'est pas recopiée entre l'écriture du journal et sa reprise ; une copie
  de l'image et de son journal ailleurs fait refuser le journal (côté sûr, dit en le nommant).
- La mise en place sans remplacement est testée par `renameat2` sous Linux glibc ; le repli par lien dur ne l'est
  que par lecture du code sur cette plateforme.

## Cartographie

À reporter dans la cartographie de l'utilisateur (`~/.claude/projects/-home-rony-projects-xteink-emulator/memory/`),
que cet agent n'a pas modifiée :

| Fichier | À mettre à jour |
|---|---|
| `architecture_overview.md` | `chips::sdcard` (carte SD en SPI, stockage par blocs : `BlockSource`, `BlockStorage`, `MemoryStorage`, `Overlay`, `SparseBlocks`) ; `host` dépend désormais de `chips` ; `host::sd_image` (journal lié à son image, verrou sur l'image, journal non appliqué fini avant la vie suivante ou vie refusée, trou percé à l'effacement sous Linux) et `host::sd_format` (mise en place sans remplacement) ; ADR 0014 ; pas de crate `storage`. |
| `backend_models.md` | Types de `chips::sdcard` : `SdCard`, `SdSlot`, `SdCardConfig`, `SdTiming` (dont `stop_ns`), `Cid`, `Capacity`, `CardKind`, `CapacityError`, `SdDiagnostic`, `SdDiagnosticKind`, `Block`, `Changes`, `StorageError` ; de `host` : `sd_image::{SdImage, FileBlocks, Access, Committed { blocks, erased_ranges, zeros_written } et notice(), SdImageError (dont UnfinishedSave, JournalTooLong, InvalidChanges), max_journal_bytes}`, `sd_format::{Fat32Layout, SdFormatError (dont Claimed, JournalPresent)}`. |
| `backend_services.md` | Modules `chips/src/sdcard/{mod, storage, registers, crc, diagnostic, tests}.rs` ; `host/src/sd_image.rs` (journal en flux, `punch_hole`, `zero_data`/`data_extents`, `sync_directory`), `host/src/sd_format.rs` (`rename_no_replace`) ; `flash_state::{sibling, open_in_place}` et `session_file::Claimed::{holds, what}` passés en `pub(crate)` ; `host` dépend de `nix` (`fs`) sous Unix. |
| `backend_controllers.md` | `inkemu sd create <chemin> --size <Mio> [--label]`, codes 0 et 2, ligne `<chemin>: <n> MiB <SDSC|SDHC/SDXC> card, FAT32, <n> clusters of <n> bytes, label <L>`. |
| `api_routes.md` | Nouvelle commande `inkemu sd create` ; tests `chips/tests/sd_driver.rs`, `host/tests/sd_card.rs`, `emu-cli/tests/sd_cli.rs`. |
| `database_schema.md` | Nouveaux fichiers : image SD `.img` (blocs bruts, taille d'une carte, écrite en place, verrouillée sur elle-même), son journal `.<nom>.journal` (`inkemu-sd-journal 2` : identité du fichier image, capacité, plages effacées, blocs écrits, SHA-256 ; `0600` ; longueur au plus `max_journal_bytes`) et `.<nom>.journal.tmp` ; image vierge créée par `.<nom>.tmp` mis en place sans remplacement, `0600`, creuse, refusée si un journal est déjà là. |
| `integrations.md` | `fatfs` 0.3.6 (MIT) en dépendance de test de `host` ; PLS 9.10 (SHA-256 `ee86b20e…`), SdFat 2.3.1 (`cda05731`) comme sources de la recherche. |
| `frontend_spa.md` | La section « Carte SD » de la boîte de démarrage est spécifiée (spec SP2e §8), à faire en SP2d tâche 15. |
| `frontend_nextjs.md` | `chips::sdcard` compile pour wasm32 ; le formatage FAT32 est dans `host` (non wasm). |
| `tech_debt.md` | Les reports du tableau « Laissé aux héritiers » ; les hypothèses de durées (dont `stop_ns`) et de CID ; le verrou consultatif ; `fatfs` non maintenu, en test seulement ; les constats de la relecture de la PR #19 sont tous corrigés (aucun report) ; effacement sans trou hors Linux (zéros, dit à l'utilisateur) tant que `unsafe` reste interdit. |
| `MEMORY.md` | Ligne d'état : SP2e fait sur sa branche (non mergé, non poussé), ADR 0014 ; intégration dans SP2d (tâches 4, 9, 11–15, 17, 18). |
