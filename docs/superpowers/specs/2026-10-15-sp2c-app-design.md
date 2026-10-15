# SP2c — L'application : la crate `host` et l'interface Tauri — design

Date : 2026-10-15. Spec globale : `docs/superpowers/specs/2026-10-11-inkemu-design.md` §1.1 (desktop et web),
§3.2 (crate `app` : « UI TypeScript partagée ; enveloppe Tauri (desktop, cœur natif) et build web (cœur WASM dans
un Web Worker) »), §6.5 (interface et langues). Recherche : `docs/research/sp2c-app-stack.md` (versions et API de
Tauri, ST ci-dessous). Héritage : ADR 0004 (entrées d'une exécution, fichier de flash), ADR 0005 (bootloader
embarqué), ADR 0006 (entrées des consoles), ADR 0007 (sessions) ; les notes de passation de SP1b-3b à SP1b-4e,
qui listent ce que la CLI a laissé « à refaire en bibliothèque ». Décision d'architecture : ADR 0010.

SP2 se fait en chantiers parallèles : SP2a (crate `chips` : SSD1677, échelle de boutons, batterie), SP2b
(périphériques du SoC), **SP2c (ce document : l'application)**, SP2d (câblage du X4, raccord de l'écran réel à
l'interface). SP2c ne touche ni `crates/soc-esp32c3` ni `crates/chips`.

## 1. But

Une version de l'émulateur qui se pilote entièrement depuis une interface graphique, avant le 2027-01-01, selon
la règle de l'utilisateur « Interface d'abord » : la logique vit dans des crates bibliothèques, la CLI et
l'interface sont minces. Deux livrables :

1. **`crates/host`**, la politique d'hôte : ce que `emu-cli` faisait autour de la machine (fichier de flash, lecture
   bornée des entrées, composition, ROM, sessions, boucle d'exécution) y migre, et la CLI la consomme sans
   changer une sortie ni un code de sortie ; plus ce que l'interface demande et que la CLI n'avait pas : une
   machine qui tourne sur un fil dédié et se commande (démarrer, pause, pas à pas, reset, arrêt), la vitesse, les
   flux des consoles, l'état de l'appareil, et deux ports pour l'écran et les boutons.
2. **`app/`**, une application Tauri 2 (TypeScript strict, Vite, React) sans logique métier, qui appelle `host`.

## 2. Décisions

| # | Décision | Raison |
|---|---|---|
| C1 | **`crates/host` reçoit les modules de politique d'`emu-cli`, déplacés tels quels** : `term`, `exit`, `flash_file`, `flash_state`, `firmware_file`, `script_file`, `session_file`, `record`, `run`, `rom_store`. `emu-cli` les réexporte (`pub use host::…`) : ses chemins publics, ses tests et ses sorties ne changent pas. Les fonctions de `main.rs` qui décident d'une exécution passent dans `host::launch` : la console montrée et sa note (`ConsoleOrigin`, `shown_console`, `console_note`), l'en-tête, la note de flash, la flash de départ (`starting_flash`), le refus des images comme fichier de flash, les chemins qu'une session ne peut pas prendre, le chargement de la ROM. Restent dans `emu-cli` : clap, Ctrl-C, la question du lecteur de la sortie standard (`reader`), `inspect`, `rvtest`. | Les trois politiques d'hôte et `flash_state.rs` que les notes de SP1b-3c à SP1b-4b demandent de déplacer ; un déplacement sans réécriture ne peut pas changer une sortie, et les 4 900 lignes de `run_cli.rs` le vérifient. |
| C2 | **Les textes restent ceux de la CLI ; l'interface rend les refus par leur variante.** Les messages des erreurs déplacées nomment des options (`--flash-file`) ; l'interface ne les montre qu'en détail, sous un titre traduit choisi par la variante (`kind`). | Spécification de l'utilisateur (notes SP1b-3c, 3d) : « rendre les refus avec ses propres mots par leur variante typée ». Changer les textes changerait la CLI. |
| C3 | **Un `Engine` sans E/S fait vivre une machine ; un fil `Emulator` le porte.** L'`Engine` (machine, ports, condensats de session, décodeurs UTF-8, état) fait une passe, un pas, reçoit les entrées ; il ne lit ni fichier ni horloge et ne crée pas de fil. Le fil (`host::device`) tient l'horloge murale (vitesse), les fichiers (flash, session), et sert les commandes. | `Esp32C3` n'est pas `Send` (`FlashDevice` n'en exige pas) : la machine naît sur le fil et n'en sort jamais, construite par une recette `Send`. Un moteur sans E/S est ce que la version web (SP6) mettra dans un Web Worker. |
| C4 | **Une vie = une mise sous tension.** Le reset de l'interface est celui de la broche `CHIP_EN` (le bouton reset du X4) : une nouvelle machine à la mise sous tension, avec la flash où l'a laissée la vie d'avant (la mémoire RTC est perdue, comme sur la puce). À la fin de chaque vie : le fichier de flash est écrit (`RunFlash::finish`), libéré et relu pour la vie suivante ; une session en cours d'enregistrement est écrite et l'enregistrement s'arrête. | `RunFlash::finish` compte les secteurs modifiés depuis la mise sous tension de la machine qu'on lui donne : sans écriture à chaque fin de vie, une vie qui modifie la flash puis un reset perdraient l'écriture. Une session couvre une mise sous tension (ADR 0007). |
| C5 | **Une attente sans fin ne termine pas une exécution de l'interface.** Un CPU qui attend une interruption qu'aucune ligne ne peut lever (`Idle`) et un sommeil profond sans réveil modélisé mettent l'appareil **en attente** : il ne tourne plus jusqu'à une entrée (octets de console, hôte USB, bouton) ou un reset, qui le relancent. Le temps d'appareil n'avance pas pendant l'attente (rien n'est planifié). Un rejeu, qui ne reçoit aucune entrée, se termine dessus comme la CLI. | La CLI termine sur ces arrêts parce que rien ne peut venir ; dans l'interface, l'utilisateur peut taper ou appuyer. |
| C6 | **Vitesse : temps réel ou au plus vite.** En temps réel, chaque passe est bornée par l'instant de temps d'appareil que l'horloge murale autorise (`run_within`, limite d'instant) ; le fil attend la suite. Au plus vite, aucune borne. Le rapport temps d'appareil / temps mural est dans l'état. | `run_within` donne la même exécution quel que soit le découpage en appels : la vitesse ne change rien à ce que fait la machine, seulement à l'instant où une entrée de l'utilisateur la trouve, et cet instant entre dans le journal. |
| C7 | **Les entrées sont datées à l'instant présent de la machine et journalisées par elle** (`send_console`, `set_usb_host`). Une entrée refusée est rendue par sa variante (`InputError`). Le dépassement des bornes du journal pendant un enregistrement est dit une fois, dès qu'il arrive. | ADR 0006 et 0007 ; note SP1b-4b (« l'interface doit le dire dès que cela arrive »). |
| C8 | **Ports de l'écran et des boutons** (§4), créés à chaque vie sur le fil, appelés entre deux passes avec la machine. Jusqu'à SP2d, une **mire de test** les implémente pour chaque profil : elle ne lit ni n'écrit rien de la machine, et l'interface le dit (bandeau). | Développer et tester toute l'interface avant que le SSD1677 soit câblé, sans prétendre montrer l'écran de l'appareil. |
| C9 | **Un bouton qui atteint le firmware ne peut pas s'enregistrer avant le format de session 2** : pendant un enregistrement ou un rejeu, l'appui sur un tel bouton est refusé (`ButtonError::NotInSession`). Les boutons de la mire n'atteignent pas le firmware et ne sont pas refusés. | ADR 0007 : les boutons rejoindront le journal avec `inkemu-session 2` (SP2d) ; une session qui les perdrait rejouerait autre chose. |
| C10 | **Enregistrer et rejouer comme la CLI.** Enregistrer arme une nouvelle mise sous tension avec l'empreinte de trace ; la session s'écrit à la fin de la vie (arrêt, reset, fin d'exécution), coupée par l'hôte (`CutReason::Interrupted`) quand l'utilisateur arrête une machine qui tourne, terminée sur `Idle` ou `DeepSleep` quand il arrête une machine en attente. Le chemin est refusé comme `--record` (`session_file::refuse_record_path`). Le rejeu reprend `record::start_replay`, `replay_options` et `verdict` : identique, divergence (première grandeur), ou coupé (rien comparé). | Une seule politique (`record.rs`) pour les deux frontaux. |
| C11 | **IPC : des commandes Tauri, un canal par émulateur, l'image à la demande.** Les mises à jour (texte des consoles, état au plus dix fois par seconde, nouvelle génération d'image, avis, fin) passent par un `tauri::ipc::Channel` ; l'image passe par une commande qui rend un `tauri::ipc::Response` (octets bruts) quand une génération est annoncée. Les types qui traversent sont ceux de `host`, sérialisés par serde en camelCase. | ST §2 : les événements ne sont pas faits pour du débit, les canaux livrent dans l'ordre, `Response` évite le JSON pour 384 000 octets. |
| C12 | **La console décodée côté hôte.** Les octets de chaque console passent par un décodeur UTF-8 en flux (une séquence coupée entre deux passes n'est pas cassée ; un octet invalide devient U+FFFD). L'interface retire les séquences ANSI de couleur et borne ce qu'elle garde (lignes). | Une seule implémentation testée ; le TypeScript ne fait que montrer. |
| C13 | **La capture d'écran est faite par `host`** : un PNG en niveaux de gris de l'image telle que l'utilisateur la voit (rotation de montage appliquée), aux pixels du panneau, sans mise à l'échelle. | Une capture exacte, indépendante de la taille de la fenêtre ; un encodeur PNG minimal sur `flate2`, déjà dépendance. |
| C14 | **La ROM** : du cache, téléchargée au premier usage comme `inkemu run` (l'avis de téléchargement devient un avis de l'interface), ou d'un répertoire donné (lecture seule). | Même politique que la CLI (`launch::load_rom`). |
| C15 | **Front** : React 19.3, Vite 8, TypeScript 7.0 strict (`noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`), Vitest 5 avec jsdom et Testing Library, Biome 2.5 pour le lint et le format ; pnpm avec lockfile versionné. Aucune logique métier : les composants montrent ce que `host` envoie et appellent ses commandes par un seul module `ipc`. | ST §1 : dernières versions stables ; `typescript-eslint` ne prend pas TypeScript 7. React : la pile de l'utilisateur, et l'interface grandira (moniteur, vue technique, débogueur). |
| C16 | **i18n FR/EN** : deux catalogues aux mêmes clés (un test le vérifie), langue système détectée (`navigator.languages`), choix gardé ; **thème** clair/sombre suivant le système, choix gardé ; **accessibilité** : rôles et noms ARIA, tout au clavier (boutons de l'appareil : touches flèches, Entrée, Retour arrière/Échap, `P` pour l'alimentation, quand l'appareil a le focus), focus visible, annonces polies de l'état. | Spec globale §6.5 ; consigne. |
| C17 | **L'enveloppe Tauri est un espace de travail Cargo à part** (`app/src-tauri`, son propre `Cargo.lock` versionné), qui dépend de `host` par chemin. | Dans l'espace de travail racine, `cargo test --workspace`, clippy et la couverture compileraient WebKitGTK sur un exécuteur qui n'en a pas les bibliothèques. |
| C18 | **CI** : un job `app` (Ubuntu 24.04) : `pnpm install --frozen-lockfile`, Biome, `tsc`, Vitest, `vite build`, puis `cargo build --locked` de l'enveloppe après les paquets système de Tauri. Ni empaquetage ni signature (SP9). | Minimal en temps, il vérifie que l'enveloppe se construit. |

## 3. La crate `host`

```
host
├── term, exit, flash_file, flash_state, firmware_file, script_file,
│   session_file, record, run, rom_store          (déplacés d'emu-cli, C1)
├── launch      politique de départ partagée par la CLI et l'interface
├── ports       PanelSource, ButtonPort et leurs types (§4)
├── pattern     la mire de test (C8)
├── catalog     le panneau et les boutons de chaque profil, jusqu'à SP2d
├── utf8        décodeur UTF-8 en flux (C12)
├── capture     PNG d'une image du panneau (C13)
├── engine      une vie de machine, sans E/S (C3)
└── device      le fil, ses commandes, la vitesse, les vies, les fichiers
```

`device::power_on(Setup, log)` suit l'ordre des refus de `inkemu run` (chemin de session, images contre fichier de
flash, flash de départ, fichier de flash, avertissements du bootloader, ROM, machine, acceptation du fichier) et
rend un `Launch` ; `Emulator::start(Launch, observer)` le fait tourner. `device::open_replay(path, rom_dir, log)`
fait de même pour un rejeu. Les commandes (`Command`) sont servies entre deux passes ; chacune rend son résultat
à l'appelant. L'observateur reçoit des `Update` dans l'ordre.

## 4. Contrat des ports (pour SP2d)

```rust
pub trait PanelSource {
    fn describe(&self) -> PanelDescription;                       // constant pendant une vie
    fn poll(&mut self, machine: &mut Esp32C3) -> Option<PanelFrame>;
}
pub trait ButtonPort {
    fn buttons(&self) -> &[ButtonSpec];
    fn reaches_firmware(&self) -> bool;
    fn set(&mut self, machine: &mut Esp32C3, button: &str, pressed: bool) -> Result<(), ButtonError>;
}
pub trait Recipe: Send {
    fn power_on(&mut self, flash: Option<Vec<u8>>) -> Result<Life, PowerOnError>;
}
pub struct Life { pub machine: Esp32C3, pub panel: Box<dyn PanelSource>, pub buttons: Box<dyn ButtonPort> }
```

- **Fil** : ports et machine naissent ensemble dans `Recipe::power_on`, sur le fil de l'émulateur, et y meurent ;
  ils n'ont pas besoin d'être `Send` (une puce peut partager un `Rc` avec le SoC).
- **`poll`** est appelé après chaque passe, chaque pas et chaque entrée, la machine arrêtée entre deux
  instructions. Il rend une image quand un rafraîchissement s'est **terminé** depuis l'appel précédent (sa
  génération a augmenté) ; plusieurs rafraîchissements entre deux appels n'en rendent que le dernier.
- **`PanelFrame`** : `width` × `height` pixels dans l'ordre de balayage natif du contrôleur, ligne par ligne, un
  octet par pixel, 0 = noir, 255 = blanc, chaque valeur sur l'un des `gray_levels` niveaux également espacés (4
  niveaux : 0, 85, 170, 255) ; `generation` (strictement croissante, à partir de 1, sur une vie) ;
  `refresh` = `Full` (le panneau passe par le noir et le blanc : l'interface le montre) ou `Partial` (seuls des
  pixels changent, sans flash) ; `region` (le rectangle rafraîchi, `None` pour tout le panneau) ; `duration_nanos`
  (la durée en temps d'appareil que le contrôleur donne au rafraîchissement, si son modèle la connaît).
- **`PanelDescription`** : largeur, hauteur, niveaux de gris, `rotation` (0, 90, 180, 270 : de l'ordre natif à ce
  que voit l'utilisateur qui tient l'appareil, dans le sens horaire), `origin` (`TestPattern` ou `Device`).
- **`ButtonSpec`** : `id` (`power`, `back`, `confirm`, `left`, `right`, `up`, `down`, … : stable, sert de clé de
  traduction), `edge` (`Top`, `Right`, `Bottom`, `Left` : où il est sur l'appareil), `order` (le long du bord).
- **`set`** : appui (`true`) ou relâchement à l'instant présent de la machine. Un nom inconnu : `Unknown`. Un port
  qui atteint le firmware doit journaliser son entrée dans la machine (format de session 2, SP2d) ; tant qu'il ne
  le fait pas, `reaches_firmware()` vaut `true` et le moteur refuse ses appuis pendant un enregistrement (C9).
- **Où brancher** : `host::catalog::ports_for(profile)` rend les ports d'un profil ; SP2d y remplace la mire du
  X4 par le SSD1677 et l'échelle de boutons, sans toucher l'interface.

## 5. L'application

- **Disposition** : barre d'outils (modèle, ouvrir un firmware ou une flash, démarrer, pause/reprendre, pas, reset,
  arrêt, vitesse, enregistrer, rejouer, capturer, thème, langue, À propos) ; au centre l'appareil (habillage, écran,
  boutons placés selon `ButtonSpec`) ; à droite l'état ; en bas les consoles (UART0, UART1, USB-Serial-JTAG ; la
  console du profil d'abord) et le journal (avis, avertissements, résumé de fin).
- **Écran** : un `canvas` à la taille du panneau tourné, mis à l'échelle entière la plus grande qui tient
  (`image-rendering: pixelated`, rien de flou) ; un rafraîchissement complet joue noir, blanc puis l'image (durée
  bornée, proportionnelle à `duration_nanos` quand il est donné) ; un partiel remplace l'image sans flash ; en
  sommeil profond l'image reste (l'encre la garde) et l'état le dit.
- **Consoles** : sortie défilante bornée, saisie d'une ligne avec la fin de ligne choisie (LF, CR, CR LF, aucune),
  état et commutateur de l'hôte USB, et la phrase « rien n'atteint cette console tant qu'un hôte n'a pas ouvert le
  port » quand l'hôte n'est pas ouvert.
- **Démarrage** : modèle (X4 par défaut), source (flash effacée, image de flash, application avec bootloader
  livré ou donné), fichier de flash facultatif (et « repartir de la source »), hôte USB au départ, enregistrement
  facultatif. Les fichiers se choisissent par les dialogues du système.

## 6. Hors périmètre

- Le câblage de l'écran et des boutons (SP2d), le format de session 2 (boutons dans le journal).
- Les scripts de console depuis l'interface, `inspect`, un profil d'un fichier de l'utilisateur, l'installation
  d'une application sur un fichier de flash qui garde NVS (comparable à une OTA).
- La vue technique, le moniteur de ressources, le débogueur, le tactile.
- L'empaquetage, la signature et la notarisation (SP9) ; la version web (SP6).

## 7. Critères d'acceptation

1. `cargo test -p emu-cli` et `cargo test -p host` passent ; aucun test de `run_cli.rs` n'est modifié pour suivre
   le déplacement ; `emu-cli` n'a plus de code de politique d'exécution.
2. Les tests de `host` font tourner une machine (ROM écrite à la main) sur son fil : démarrage, pause, pas, reset
   (nouvelle vie, flash gardée), arrêt, vitesse, console dans les deux sens (UTF-8 coupé compris), attente puis
   reprise sur une entrée, enregistrement puis rejeu identique, rejeu coupé, fichier de flash écrit à chaque fin de
   vie, ports appelés, refus typés.
3. L'application se construit (`vite build`, `cargo build` de l'enveloppe), `tsc` strict, Biome et Vitest passent ;
   les deux catalogues ont les mêmes clés.
4. Le job `app` de la CI fait tout cela sans empaquetage ni signature.
