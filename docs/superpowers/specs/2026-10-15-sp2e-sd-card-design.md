# SP2e — La carte SD du X4 : design

Date : 2026-10-15. Point de départ : `main` à `4b02a91`. SP2b (PR #18) et SP2d (spec et plan faits) ne sont pas
fusionnés : ce document s'appuie sur l'état de leurs branches et dit, au §9, ce que SP2d doit intégrer et dans
quelle tâche. Spec globale : `docs/superpowers/specs/2026-10-11-inkemu-design.md` §1.1 (« carte SD : dossier hôte et
image `.img` »), §2 (« SD en SPI sur le bus de l'écran »), §3.1 (machine déterministe), §3.2 (`chips` : « carte SD
(SPI et SDMMC) » ; les puces ne voient que des signaux ; une crate `storage` y était prévue, voir 2e-D2). Recherche :
`docs/research/sp2e-sd-card.md` (RS ci-dessous) ; câblage : `docs/research/sp2d-x4-wiring.md` (RW). Contrats repris :
ADR 0003 (temps de l'appareil), 0004 (fichier de flash), 0007 (sessions), 0011 (crate `host`), 0012 (signaux, SP2b),
0013 (entrées de la carte, SP2d). ADR de ce sous-projet : 0014.

## 1. But

Avant le 2027-01-01, CrossPoint 1.6.5 atteint son écran d'accueil dans l'application de bureau. Il s'arrête
aujourd'hui sur « SD card error » parce qu'il monte la carte SD avant tout autre écran (RW §1 point 10, RS §2 point
8). SP2e livre ce qui manque, sans toucher au SoC ni au câblage :

1. **une carte SD en mode SPI** au niveau des octets (`chips::sdcard`), qui ne connaît pas le SoC, compte en temps
   d'appareil et compile pour `wasm32` ;
2. **un stockage par blocs** abstrait, en mémoire, en surcouche copie-sur-écriture d'une source en lecture seule, et
   creux (pour les sessions) ;
3. **le fichier image de l'hôte** (`host::sd_image`) : ouverture verrouillée, écriture à la fin de chaque vie par un
   journal écrit dans un fichier temporaire renommé, reprise d'un journal interrompu, refus des chemins revendiqués ;
4. **une image vierge** formatée en FAT32 par notre code (`host::sd_format`), et la commande `inkemu sd create` ;
5. la forme exacte de la carte dans une session (format 2 de SP2d) et dans l'interface, et la liste des tâches de
   SP2d qui les intègrent.

**Ce que SP2e ne donne pas seul :** brancher la carte sur les broches du X4 (adaptateur `SpiTarget`, profil,
machine câblée), la porter dans une session et dans l'interface : cela dépend des traits de SP2b et du câblage de
SP2d, qui ne sont pas fusionnés. Ce document en fixe l'interface ; SP2d l'intègre (§9).

## 2. Décisions

| # | Décision | Raison |
|---|---|---|
| 2e-D1 | **Interface au niveau des octets**, temps passé à chaque appel : `select(now, actif)`, `exchange(now, mosi) -> Option<u8>` (`None` : la carte ne pilote pas MISO, le pad lit son tirage), `transfer(now, mosi, miso)` qui écrase les octets que la carte pilote. `now` est un instant en cycles d'une base donnée à la construction (`Frequency`), comme le SSD1677 (2a-A1) ; câblée, la base vaut 1 GHz (2d-D2). Un instant antérieur au dernier vu vaut le dernier. | L'adaptateur `SpiTarget` de SP2d (`select`, `transfer` avec `miso` prérempli du tirage) se réduit à une boucle ; la carte reste testable sans SoC. |
| 2e-D2 | **Où vit le code.** Le modèle et le stockage par blocs sont dans `chips::sdcard` (aucune E/S, `wasm32`) ; le fichier image, sa reprise et le formatage FAT32 dans `host`. Pas de crate `storage` : la spec globale la prévoyait, mais une crate de plus touche le garde `host_io_ban` de `boards`, hors du périmètre ; le formatage, calcul pur, reste dans `host` jusqu'à la version web (SP6), qui le déplacera s'il lui faut. | Consigne de SP2e ; ADR 0014. |
| 2e-D3 | **La carte ne voit que ses sélections** (`SpiTarget`, ADR 0012) : les 74 coups d'horloge CS haut que SdFat envoie avant CMD0 (RS §2 point 4) ne lui parviennent pas ; elle ne les exige pas. | RS §4 point 1. |
| 2e-D4 | **Le protocole modélisé** est celui du chapitre 7 de la PLS 9.10 : passage en mode SPI par CMD0 avec CS bas et CRC valide (un CMD0 au CRC faux et toute autre commande avant sont ignorés, MISO non piloté) ; CRC désactivé par défaut, toujours vérifié pour CMD8, pour tout après CMD59 (commandes et blocs écrits) ; réponse au **deuxième** octet qui suit la commande (le premier est `FFh`) ; états repos, initialisation (ACMD41), prêt ; commandes CMD0, 1, 6, 8, 9, 10, 12, 13, 16, 17, 18, 24, 25, 32, 33, 38, 55, 58, 59 et ACMD13, 22, 23, 41, 42, 51 ; réponses R1, R1b, R2, R3, R7 ; jetons `FEh`, `FCh`, `FDh`, réponse de données, jeton d'erreur ; une ACMD non définie est la commande ordinaire de même numéro (PLS §4.3.9.1). | RS §1. CMD59, CMD6, ACMD13 et ACMD51 servent au pilote SD d'ESP-IDF, que le firmware de référence d'une étape suivante utilisera. |
| 2e-D5 | **Non modélisé, dit par un diagnostic nommé** et répondu « commande illégale » : verrouillage (CMD42), groupes de protection (CMD28–30), CMD27, CMD56, les ACMD de sécurité (18, 25, 26, 38, 43–49). Le CSD déclare les classes 0, 2, 4, 5, 8, 10 (`535h`), sans la classe 7 que la PLS fixe pour SDHC : annoncer le verrouillage serait annoncer ce qui n'existe pas. Toute commande d'un numéro inconnu : « commande illégale » et diagnostic `UnknownCommand`. | Spec globale §7 : ne jamais cacher une erreur. RS §5. |
| 2e-D6 | **Le type de carte suit la capacité** : jusqu'à 2 Gio, SDSC (CSD 1.0, adresses en octets, CMD16 de 1 à 512 en lecture, 512 seulement en écriture, aucun accès à cheval sur un bloc de 512) ; au-delà, SDHC/SDXC (CSD 2.0, adresses en blocs de 512). Une capacité qui ne se code pas dans le CSD est refusée par `Capacity::new`, avec la règle à suivre (un multiple d'1 Mio la code toujours jusqu'à 2 Gio ; au-delà, un multiple de 512 Kio, au plus 2 Tio − 128 Mio). Une SDHC exige CMD8 puis HCS = 1 à son premier ACMD41, sinon elle reste au repos (diagnostic `HighCapacityWithoutHcs`). | PLS §5.3, Figure 7-1 note 1 ; RS §5. |
| 2e-D7 | **Les durées** sont des paramètres de la carte (`SdTiming`), en nanosecondes de temps d'appareil, jamais l'horloge de l'hôte : initialisation 50 ms après le premier ACMD41, accès en lecture 250 µs (avant chaque jeton de données), programmation 1 ms par bloc écrit, effacement 2 ms, fin d'une écriture multiple 250 µs (après le jeton `FDh`, PLS Figure 7-7, et après CMD12 qui l'arrête, R1b) ; CMD12 qui arrête une lecture n'est suivi d'aucune occupation (PLS §4.9.2 : « based on its state prior to the command reception » ; rien n'est à programmer). Pendant une occupation (programmation, effacement), la carte sélectionnée tient MISO à `00h` et ignore les commandes (diagnostic) ; désélectionnée, elle relâche MISO et continue (PLS §7.2.4). | RS §5 : les temps d'accès sont retirés de la PLS simplifiée ; ces valeurs tiennent dans les délais de SdFat et de la PLS (§4.6.2). |
| 2e-D8 | **Une désélection** coupe ce qui est en cours d'octets : une commande partielle est perdue, un bloc en cours d'envoi ou de réception est abandonné (diagnostic `TransferCut`), une lecture de plusieurs blocs s'arrête ; ce qui n'est pas encore sorti d'une réponse est perdu. L'attente du jeton suivant d'une écriture de plusieurs blocs et l'occupation se poursuivent. | PLS §7.2 (« la carte compte les coups d'horloge à partir de CS ») et §7.2.4 ; RS §5. |
| 2e-D9 | **Protection en écriture** : `SdCardConfig::write_protected` met `TMP_WRITE_PROTECT` dans le CSD ; une écriture reçoit la réponse de données « erreur d'écriture », CMD13 montre « WP violation » ; un effacement ne fait rien et montre « WP erase skip ». Une image ouverte en lecture seule donne une carte protégée. | PLS §7.2.4, §7.3.3.1 ; RS §1 point 13. |
| 2e-D10 | **Aucune panique depuis les octets du firmware** : aucune indexation non vérifiée, arithmétique d'adresse vérifiée, tampons de taille fixe ; les diagnostics sont bornés (1 024 gardés, les suivants comptés). Une erreur du stockage (lecture ou écriture d'un fichier de l'hôte) devient un jeton d'erreur de données ou une réponse « erreur d'écriture » et le bit « error » de R2, avec un diagnostic qui cite le stockage. | Spec globale §7 ; 2a-A5. |
| 2e-D11 | **Pas de carte** : `SdSlot::empty()` ne pilote jamais MISO (`None` à chaque octet) ; avec un tirage haut, le pilote lit `FFh`, SdFat abandonne CMD0 après 2 s, et CrossPoint affiche « SD card error ». Le logement porte ou non une carte ; insérer et retirer pendant une vie relève de SP4. | RS §4 point 2. |
| 2e-D12 | **Le stockage par blocs** est un trait (`BlockStorage` : nombre de blocs, lire, écrire, effacer une plage) avec trois implémentations dans `chips` : `MemoryStorage` (tout en mémoire), `Overlay<S: BlockSource>` (copie sur écriture d'une source en lecture seule, plages effacées gardées comme plages, changements itérés dans l'ordre des blocs) et `SparseBlocks` (source creuse : les blocs non nuls d'une session). | Une image de 32 Gio ne tient pas en mémoire ; une vie n'écrit que quelques Mio ; un effacement de toute la carte ne doit pas coûter 32 Gio de mémoire. |
| 2e-D13 | **Le fichier image** (`host::sd_image::SdImage`) : fichier ordinaire (lien non suivi), taille codable (2e-D6), ouvert une fois par exécution et **verrouillé** (verrou exclusif sur le fichier lui-même en lecture-écriture, partagé en lecture seule : le fichier n'est jamais remplacé, il est écrit en place, donc son inode porte le verrou ; pas de fichier `.lock`). Une vie lit ses blocs à la demande et écrit en surcouche ; **à la fin de chaque vie**, les changements partent dans un journal `.<nom>.journal` écrit d'abord sous `.<nom>.journal.tmp` (mode `0600`, vidé sur disque), renommé, puis appliqués en place, vidés, et le journal effacé. Un journal trouvé à l'ouverture (sauvegarde interrompue) est vérifié par son SHA-256 et appliqué, ou refusé en le nommant ; un temporaire trouvé est d'une sauvegarde qui n'a pas eu lieu : effacé et dit. Un journal dont l'application a échoué reste, et passe avant tout : `source` et `commit` le réappliquent d'abord, ou refusent la vie (`UnfinishedSave`) ; une sauvegarde n'écrit jamais par-dessus. Le journal (`inkemu-sd-journal 2`) porte l'identité du fichier image (périphérique et inode sous Unix, SHA-256 du chemin canonique ailleurs), que son condensat couvre ; sa longueur est bornée par celle du plus long journal de la carte avant toute lecture, il est lu en flux, et son type revérifié sur le descripteur ouvert. Les changements sont vérifiés (blocs et plages dans la carte) avant d'écrire le journal. Un effacement perce un trou sous Linux ; ailleurs des zéros vont sur les seules données (`SEEK_DATA`), et `Committed::notice` le dit. | Le fichier de flash est remplacé entier (ADR 0004) ; une image de plusieurs Gio ne peut pas être recopiée à chaque fin de vie. Le journal donne la même garantie qu'un fichier temporaire renommé : après une interruption, l'image est celle d'avant ou celle d'après, jamais un mélange. ADR 0014. |
| 2e-D14 | **Chemins revendiqués** : `SdImage::open` refuse un chemin que l'exécution lit ou écrit déjà (`session_file::Claimed` : fichier de flash, son temporaire, son verrou, images d'entrée, session, ROM) ; `files_of_the_sd_image` donne l'image, son journal et son temporaire, que les autres refus ajoutent à leur liste. | Même règle que le fichier de flash et la session (ADR 0004 point 5, ADR 0007 point 9). |
| 2e-D15 | **Une image vierge** : FAT32 sur une table MBR d'une partition de type `0Ch` à 8 192 secteurs, zone de données alignée sur 8 192 secteurs, deux FAT, secteur de boot, FSInfo et leurs copies (secteurs 6 et 7), répertoire racine d'un cluster avec l'entrée d'étiquette de volume ; taille des clusters : la plus grande puissance de deux jusqu'à 32 Kio qui laisse au moins 65 525 + 16 clusters ; numéro de série et date fixes (pas d'horloge, deux créations donnent les mêmes octets). Taille en Mio, de 64 Mio à 2 Tio − 128 Mio. Le fichier est **creux** (`set_len`, seules les structures écrites), écrit sous un nom temporaire en `0600`, vidé, puis renommé ; un chemin existant est refusé. Notre propre code : `fatfs`, la seule crate de formatage FAT répandue, n'a pas de version depuis janvier 2023 ; elle sert seulement, en dépendance de test, à relire l'image que nous formatons. | RS §2 point 9 (ce que SdFat exige), RS §3 (un volume vide suffit à CrossPoint, qui crée `/.crosspoint` lui-même). |
| 2e-D16 | **`inkemu sd create <chemin> --size <Mio> [--label <texte>]`** : la CLI mince appelle `host::sd_format::create_blank` ; codes 0 (créée) et 2 (refus d'argument ou de chemin, écriture impossible : le code 2 de la CLI dit déjà « entrée refusée ou problème d'environnement », et 4 a un sens propre à `run`). | Préférence « interface d'abord, CLI mince » : la logique est dans `host`, l'interface l'appellera (§8). |
| 2e-D17 | **La carte dans une session** (forme à intégrer par SP2d, §7) : le contenu de départ en blocs non nuls (plages maximales, compressées), sa capacité, la protection, et l'empreinte de cette forme ; le résultat comparé gagne l'empreinte des changements de la vie. Borne : 64 Mio de blocs non nuls ; au-delà, l'enregistrement est refusé au départ, en le disant. | ADR 0007 : une session se rejoue seule, sans chemin de l'hôte. Une image vierge de 32 Gio n'a que quelques Mio non nuls ; une empreinte sur l'image entière coûterait la lecture de toute l'image à chaque fin de vie. |

## 3. Architecture

```
chips::sdcard        SdCard<S: BlockStorage>  select, exchange, transfer ; SdSlot<S> (vide ou une carte)
  storage            BlockStorage, BlockSource, MemoryStorage, Overlay<S>, SparseBlocks, Changes, StorageError
  registers          Capacity, CardKind, Cid, csd(), scr(), ocr(), switch_status()
  crc                crc7, crc16 (PLS §4.5)
  diagnostic         SdDiagnostic, SdDiagnosticKind
host::sd_image       SdImage (ouvrir, verrouiller, reprendre, source(), commit()), FileBlocks : BlockSource,
                     files_of_the_sd_image, SdImageError
host::sd_format      Fat32Layout (calcul pur), create_blank (fichier creux, temporaire renommé)
emu-cli              inkemu sd create
```

**Flux d'une lecture de secteur par CrossPoint (après intégration SP2d)** : SdFat abaisse GPIO12 → la machine appelle
`SpiTarget::select(true)` de l'adaptateur → `SdSlot::select(now, true)` ; SdFat transfère `52 00 00 20 00 87` (CMD18
au bloc 0x2000) puis lit → `SdSlot::transfer(now, mosi, miso)` : MISO `FF`, puis R1 `00` au deuxième octet ; SdFat
attend un octet autre que `FF` → après 250 µs de temps d'appareil, `FE`, les 512 octets lus de `Overlay` (bloc changé,
sinon plage effacée, sinon `FileBlocks` qui lit l'image), le CRC16 ; SdFat envoie CMD12 → la carte arrête le flux,
`FF` puis R1 `00` ; GPIO12 remonte → `select(false)`.

**Flux d'une fin de vie** : le fil de l'émulateur prend la surcouche de la carte (`SdSlot::take` puis
`SdCard::into_storage`), `SdImage::commit(&overlay.changes())` écrit le journal, l'applique, l'efface ; la vie
suivante ouvre une nouvelle surcouche sur la même image.

## 4. L'interface du modèle (pour SP2d)

```rust
// chips::sdcard
pub const BLOCK_BYTES: usize = 512;
pub type Block = [u8; BLOCK_BYTES];
pub trait BlockSource: std::fmt::Debug {
    fn block_count(&self) -> u64;
    fn read_block(&mut self, lba: u64, into: &mut Block) -> Result<(), StorageError>;
}
pub trait BlockStorage: BlockSource {
    fn write_block(&mut self, lba: u64, data: &Block) -> Result<(), StorageError>;
    fn erase(&mut self, first: u64, count: u64) -> Result<(), StorageError>; // remet à zéro
}
pub struct MemoryStorage;  // new(blocks), from_bytes(Vec<u8>) -> Result, bytes(), into_bytes()
pub struct Overlay<S: BlockSource>; // new(base), changes() -> Changes<'_>, base(), into_base()
pub struct Changes<'a> { /* erased(): &[(u64, u64)], blocks(): impl Iterator<Item = (u64, &Block)> */ }
pub struct SparseBlocks;   // new(block_count), insert(lba, Block) -> Result
pub struct Capacity;       // new(bytes) -> Result<Capacity, CapacityError>, bytes(), blocks(), kind()
pub enum CardKind { Sdsc, Sdhc }
pub struct SdTiming { pub initialization_ns, pub read_access_ns, pub program_ns, pub erase_ns: u64 }
pub struct Cid { pub manufacturer: u8, pub oem: [u8; 2], pub product: [u8; 5], pub revision: u8,
                 pub serial: u32, pub year: u16, pub month: u8 }
pub struct SdCardConfig { pub cid: Cid, pub write_protected: bool, pub timing: SdTiming } // Default
pub struct SdCard<S: BlockStorage>;
impl<S: BlockStorage> SdCard<S> {
    pub fn new(config: SdCardConfig, storage: S, time_base: Frequency) -> Result<Self, CapacityError>;
    pub fn select(&mut self, now: Cycles, selected: bool);
    pub fn exchange(&mut self, now: Cycles, mosi: u8) -> Option<u8>;
    pub fn transfer(&mut self, now: Cycles, mosi: &[u8], miso: &mut [u8]);
    pub fn capacity(&self) -> Capacity;
    pub fn storage(&self) -> &S;
    pub fn into_storage(self) -> S;
    pub fn take_diagnostics(&mut self) -> Vec<SdDiagnostic>;
    pub fn dropped_diagnostics(&self) -> u64;
}
pub struct SdSlot<S: BlockStorage>; // empty(), holding(card), select, exchange, transfer, card(), take()
```

L'adaptateur de SP2d, `SdLines` (`SpiTarget`), tient un `SdSlot<Overlay<…>>` et appelle `select` et `transfer` avec
l'instant `Nanos` comme `Cycles` d'une base d'1 GHz ; la MISO de la carte n'est ni sur un `GpioOutput` ni sur un
`GpioInput` (le X4 ne fait pas de bit-bang sur la carte) ; la carte n'a pas de temps propre (`Clocked`) : rien ne
change sur ses lignes sans que le maître horloge, et une occupation se lit au prochain octet.

## 5. L'interface du fichier image (pour SP2d)

```rust
// host::sd_image
pub enum Access { ReadWrite, ReadOnly }
pub struct SdImage;
impl SdImage {
    pub fn open(path: &Path, access: Access, claimed: &[Claimed], log: &mut dyn Write)
        -> Result<SdImage, SdImageError>; // vérifie, verrouille, reprend un journal
    pub fn shown(&self) -> &str;
    pub fn capacity(&self) -> Capacity;
    pub fn write_protected(&self) -> bool;           // Access::ReadOnly
    // une par vie ; finit d'abord une sauvegarde inachevée (UnfinishedSave sinon)
    pub fn source(&mut self) -> Result<FileBlocks, SdImageError>;
    pub fn commit(&mut self, changes: &Changes<'_>) -> Result<Committed, SdImageError>;
}
pub struct Committed { pub blocks: usize, pub erased_ranges: usize, pub zeros_written: u64 }
impl Committed { pub fn notice(&self) -> Option<String>; } // zéros écrits faute de trou : à dire
pub fn files_of_the_sd_image(path: &Path) -> Vec<(PathBuf, &'static str)>;
pub fn max_journal_bytes(blocks: u64) -> u64;
// host::sd_format : `claimed` = les chemins que l'exécution revendique (vide pour la CLI)
pub fn create_blank(path: &Path, mebibytes: u64, label: &str, claimed: &[Claimed])
    -> Result<Fat32Layout, SdFormatError>;
```

Une vie : `SdCard::new(SdCardConfig { write_protected: image.write_protected(), ..Default::default() },
Overlay::new(image.source()?), Frequency(1 GHz))`. Un rejeu : `Overlay::new(SparseBlocks)` reconstruit de la session,
jamais le fichier.

## 6. Tests et critères d'acceptation

1. **Modèle** (`chips`, unitaires, sans SoC) : un test par commande et par état — mode SD (CMD0 au CRC faux ignoré,
   autre commande ignorée, MISO non piloté), CMD0, CMD8 (CRC faux, tension refusée, écho), ACMD41 (initialisation à
   son instant, HCS absent sur SDHC), CMD58 (avant et après), CMD59 et CRC des commandes et des blocs, CMD9, CMD10,
   CMD13 (bits effacés à la lecture), CMD16 (SDSC, SDHC, hors bornes), CMD17 (SDSC partiel, à cheval refusé, hors
   carte), CMD18 et CMD12, CMD24 et CMD25 (jeton `FCh`, `FDh`, occupation, carte protégée), CMD32/33/38 (séquence,
   protection), CMD6, ACMD13, ACMD22, ACMD23, ACMD42, ACMD51, ACMD non définie, commandes non modélisées et inconnues,
   commande pendant l'occupation et pendant une lecture, désélections au milieu de chaque phase, octets parasites,
   bornes des diagnostics, aucune panique sur des octets quelconques (une suite pseudo-aléatoire déterministe de
   plusieurs centaines de milliers d'octets et de sélections).
2. **Stockage** : `MemoryStorage`, `Overlay` (lire à travers, écrire, effacer une plage puis écrire dedans, changements
   dans l'ordre, effacement de 2³² blocs sans mémoire), `SparseBlocks`.
3. **De bout en bout** : un pilote de test qui suit la séquence de SdFat 2.3.1 (RS §2 points 4 à 7) initialise une
   SDSC et une SDHC, lit et écrit des blocs seuls et en série, et relit ce qu'il a écrit.
4. **Fichier image** (`host`) : ouverture (lien, taille non codable, fichier absent, chemin revendiqué, verrou tenu
   par une autre ouverture, lecture seule), lecture à travers une vie, `commit` puis réouverture, journal interrompu
   avant et après le renommage, journal corrompu refusé, image changée de taille entre deux vies refusée ;
   application qui échoue (point d'injection propre aux tests) puis vie suivante qui lit la vie précédente entière,
   sauvegarde qui ne peut être finie refusée sans toucher au journal, journal d'une autre image de même taille
   refusé, journal creux de 1 Tio refusé avant lecture, journal aussi long que la carte le permet accepté,
   changements hors carte refusés sans journal, effacement de toute la carte qui libère sa place (Linux), zéros sur
   les seules données en repli, journal lien, tube (avant et après le regard) et temporaire lien, lecture seule qui
   ne touche à rien.
5. **Image vierge** : géométrie (clusters ≥ 65 541, alignements, copies), octets identiques d'une création à
   l'autre, fichier creux, refus d'un chemin existant, d'un journal laissé par une ancienne image et d'un chemin
   revendiqué, mise en place qui ne remplace jamais un fichier, relue par la crate `fatfs` (racine vide, étiquette, création
   d'un dossier et d'un fichier relus), montée par le pilote de test à travers le modèle, en SDSC et en SDHC.
6. **CLI** : `inkemu sd create` crée, refuse une taille hors bornes et un chemin existant.
7. **Local, après intégration SP2d** : CrossPoint 1.6.5 avec une image vierge atteint son écran d'accueil ; la
   mention est dans la note de passation de SP2d (tâche 17).

## 7. La carte dans une session (forme proposée pour le format 2)

SP2d définit le format 2 (`inkemu-session 2`, entrée d'étiquette 4 pour les boutons, 2d-D10). La carte y entre dans
**la même version**, pour ne pas changer deux fois le numéro. Après les entrées de la flash, avant les options :

| Champ | Type | Sens |
|---|---|---|
| présence | `u8` | 0 : pas de carte (logement vide) ; 1 : une carte, les champs suivants |
| capacité | `u64` | en blocs de 512 ; refusée si `Capacity::new` la refuse |
| protégée | `u8` | 0 ou 1 |
| plages | `u32` | nombre de plages de blocs non nuls, au plus 131 072 |
| chaque plage | `u64` premier bloc, `u32` nombre | triées, disjointes, non contiguës, non vides, dans la capacité (forme canonique : un lecteur refuse toute autre) |
| données | `u32` longueur, octets | les blocs des plages, dans l'ordre, compressés (deflate) ; décompressés : 512 × somme des nombres, au plus 64 Mio |
| empreinte | 32 octets | SHA-256 de la capacité (`u64`), puis de chaque plage (`u64`, `u32`) et de ses octets, petit-boutiste |

Le résultat (`Outcome`) gagne, après l'empreinte de la flash finale, `u8` présence puis le SHA-256 des
**changements** de la dernière vie (`Changes` : chaque plage effacée en `u64` premier, `u64` nombre, puis chaque bloc
changé en `u64` numéro et ses 512 octets, dans l'ordre). Bornes : `MAX_SD_NONZERO_BYTES` = 64 Mio ;
`MAX_SESSION_BYTES` augmente d'autant (et de la marge de deflate, comme pour la flash). Un enregistrement dont l'image
de départ dépasse la borne est refusé **avant l'exécution** (`RecordError::SdCardTooFull`, avec la taille trouvée et
la borne). Le rejeu construit `Overlay::new(SparseBlocks)` et ne lit jamais l'image de l'hôte.

## 8. L'interface

Dans la boîte de démarrage, une section « Carte SD » / « SD card » :

- un groupe de boutons radio « Aucune carte » / « No card » (par défaut), « Image existante » / « Existing image »,
  « Nouvelle image » / « New image », dans un `fieldset` dont la `legend` est « Carte SD » / « SD card » ;
- « Image existante » : le `PathField` de SP2c (bouton « Choisir… », nom du fichier, « Retirer ») avec le filtre
  `img`, et une case « Protégée en écriture » / « Write-protected » ;
- « Nouvelle image » : un champ de chemin d'enregistrement (`pickSavePath("sdImage")`), un champ numérique « Taille
  (Mio) » / « Size (MiB) », `min` 64, `step` 1, valeur 1 024, `aria-describedby` vers l'aide « De 64 Mio à 2 Tio ;
  le fichier ne prend sur le disque que ce qui est écrit » / « From 64 MiB to 2 TiB; the file takes on disk only
  what is written » ; la création se fait au démarrage, avant la mise sous tension, et son refus s'affiche dans la
  bannière d'erreur (`role="alert"`) avec le message de `SdFormatError` ;
- l'état de l'appareil montre « Carte SD : nom.img (1 024 Mio, SDSC) » / « SD card: name.img (1,024 MiB, SDSC) », ou
  « Carte SD : aucune » ; une image ouverte en lecture seule ajoute « protégée en écriture ».

Tout l'est au clavier (ordre de tabulation du formulaire), les libellés sont des `label` liés à leur champ, les
erreurs sont annoncées. L'implémentation dépend de la machine câblée de SP2d (le `Setup` doit atteindre une carte) :
elle est confiée à la tâche 15 de SP2d (§9) ; SP2e livre le calcul et le fichier côté `host`.

## 9. Ce que SP2d doit intégrer, tâche par tâche

| Tâche de SP2d | Ajout |
|---|---|
| 0 (rebase) | lire ce document, ADR 0014 et la note de passation de SP2e ; amender le §9 de la spec de SP2d s'il change quoi que ce soit |
| 4 (puces analogiques, `chips`) | `chips::sdcard::lines::SdLines` : `SpiTarget` pour un `SdSlot<S>` (base 1 GHz) ; tests : une sélection sans transfert ne fait rien, `miso` prérempli `FF` revient `FF` pour un logement vide, une initialisation par transferts égale celle par `exchange` |
| 9 (profil) | `[wiring.sd] cs = 12, miso = 7` (SCLK et MOSI ceux de l'écran), refus d'une broche prise par une autre puce sauf SCLK/MOSI partagés avec l'écran ; X3 sans carte jusqu'à SP3 |
| 11 (machine câblée) | `WiredOptions.sd: Option<SdStart>` (`SdStart { storage, write_protected }`) ; `Wired::power_on` attache `SdLines` sur CS 12 et MISO 7 ; poignée `SdHandle` qui rend la surcouche à la fin de la vie ; test : une carte vide n'empêche pas l'écran, une carte présente répond à CMD0 par la machine |
| 12 (session) | le §7 ci-dessus : champs, bornes, forme canonique, refus nommés, `SparseBlocks` au rejeu, empreinte des changements dans le résultat ; tests : session avec carte rejouée en tranches de 1, 7, 997 et 7 919 cycles |
| 13 (CLI) | `inkemu run --sd-image <chemin> [--sd-read-only]` : `SdImage::open` avec la liste revendiquée, commit à la fin ; `--record` avec une carte ; `files_of_the_sd_image` dans la liste des chemins revendiqués par `--record`, `--flash-file` et la capture ; ligne de résumé `sd card: <nom> (<Mio> MiB, SDSC/SDHC), <n> blocks written` |
| 14 (ports, `host`) | `Setup.sd: Option<SdSetup>` (`Existing { path, read_only }` ou `New { path, mebibytes }`) ; `device.rs` ouvre l'image au démarrage (après `create_blank` pour une nouvelle, qui reçoit les mêmes chemins revendiqués que `SdImage::open`), commit à chaque fin de vie, et le dit comme pour la flash (avec `Committed::notice` quand des zéros ont été écrits) ; `source` prend `&mut self` et peut refuser la vie (`UnfinishedSave`) : la vie ne démarre pas avec la carte, l'erreur est dite ; la vie éteinte et le rallumage gardent l'image ouverte |
| 15 (interface) | le §8 ci-dessus, en FR et en EN, avec ses tests Vitest (affichage par source, création refusée annoncée, état de l'appareil) |
| 17 (firmwares réels, local) | CrossPoint 1.6.5 avec une image vierge de 1 024 Mio (`inkemu sd create`) : l'écran d'accueil est publié ; le test vérifie que l'image n'est pas « SD card error », écrit un PNG, et que `/.crosspoint/state.json` existe dans l'image après la vie (relue par `fatfs` en test) |
| 18 (ADR, documents) | ADR 0013 renvoie à ADR 0014 pour la carte ; la note de SP2d reprend les héritiers de SP2e (dossier hôte en FAT virtuel, insertion à chaud) |

## 10. Hors périmètre, et héritiers

| Sujet | Héritier |
|---|---|
| Dossier de l'hôte exposé en FAT32 virtuel (spec globale §1.1) | sous-projet suivant la livraison du 2027-01-01 (SP4 ou dédié) : un `BlockSource` qui synthétise le volume |
| Insérer, retirer, protéger la carte pendant une vie | SP4 (`SdSlot::take`, `SdSlot::insert` existent) |
| exFAT pour une image vierge | à la demande ; SdFat le lit déjà (RS §2 point 9) |
| SDMMC (X4 Pro, X4C) | SP5 (ESP32-S3) |
| Verrouillage par mot de passe, CPRM, groupes de protection | si un firmware en dépend |
