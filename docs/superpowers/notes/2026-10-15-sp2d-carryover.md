# SP2d — Note de passation

Branche de SP2d, partie de `main` à `6de74fb` (fusion de SP2e). Rien n'est poussé. Spec :
`docs/superpowers/specs/2026-10-15-sp2d-x4-wiring-design.md` (décisions 2d-D1 à 2d-D23) ; plan :
`docs/superpowers/plans/2026-10-15-sp2d-x4-wiring.md` (règles 2d-R1 à 2d-R12, tâches 0 à 18) ; recherche :
`docs/research/sp2d-x4-wiring.md` ; ADR 0013 (et ADR 0014 de SP2e, que SP2d intègre).

## Résultat

Le X4 est câblé de bout en bout : son écran, ses six boutons d'échelle, son bouton et son verrou d'allumage, sa
batterie, son câble et sa carte SD sont sur les broches de son profil, ses entrées entrent dans les sessions, et
l'application de bureau montre l'écran du firmware et lui transmet ses boutons.

| Commit | Contenu |
|---|---|
| `bf4dfbf` | recherche : broches, polarités, échelles, alimentation, orientation (FreeInk `111fdcc7`, CrossPoint 1.6.5) |
| `3ff36b3`, `46e7f84` | spec et plan ; §9 de la spec vérifié contre SP2b fusionné (`0a04195`) et les crochets que SP2e demande |
| `b147f5f` | `emu-core` : `BoardInput`, `BoardInputs`, `Supply` |
| `8e90bda` | `chips` : le SSD1677 sur ses lignes (cible SPI, cadencé, RES#, BUSY et sa chute annoncée, bus en bit-bang et lecture demi-duplex) ; `PanelImage.started` |
| `3a21607` | `chips` : échelles et batterie en sources analogiques, bouton poussoir |
| `3a6b305` | `soc-esp32c3`, `session` : entrées de la carte datées, journalisées, appliquées ; alimentation et `PowerOff` ; format 2 |
| `3409654`, `5045a05` | `soc-esp32c3` : réveil du sommeil profond par une broche, maintien des pads, lus dans ESP-IDF 5.5.2 (2d-R7) |
| `ca6b59e` | `console-script` : une extinction finit un script comme un sommeil profond |
| `2fcd362`, `eaf8716` | `chips` : le lecteur SD sur ses lignes (`SdLines`, 1 GHz) ; source de blocs en boîte |
| `de59dba` | `boards` : le câblage dans le profil (`[wiring]`, `[power]`), vérifié clé par clé |
| `a3a9956` | `boards` : `Wired::power_on`, la machine câblée et ses poignées ; geste d'allumage |
| `30824f3` | `session` : la carte SD dans le format 2 ; rejeu sur la machine câblée |
| `31a4487` | CLI : `--power-hold`, `--usb-power`, `--sd-image`, `--sd-read-only` ; `press`/`release` dans les scripts |
| `e888507` | `host` : les ports du X4 câblé (`ports_wired`), la carte SD d'une exécution, le bouton d'allumage qui rallume ; geste de 1,5 s ; tests locaux des firmwares réels par le chemin de l'interface |
| `d9d526e`, `4a6b97d` | `app` : teintes continues (image v2), mention « Rendu approché », état éteint, câble USB, section « Carte SD » ; borne de taille corrigée (2 097 024 Mio) |
| `0744f1f`, `2c0d625` | firmware de référence `x4_panel_buttons`, ses tests `boards` et `run_cli`, job CI `reference-boot` ; images comparées octet par octet |
| `991bc25` | ADR 0013 ; README (anglais et français) |
| `f4a4a21` … `870286a` | corrections de la relecture de la PR #20 (section suivante) |
| `34e97a4` … `2f645ab` | corrections de la seconde relecture (R1 à R8, section « Seconde relecture ») |

## Relecture de la PR #20

Quatorze constats (émulation E1 à E5, hôte, CLI et application H1 à H9), un correctif chacun, avec un test qui échoue
sans lui (vérifié en neutralisant le correctif puis en le rétablissant). Plus un compte de la CI.

| Commit | Constats |
|---|---|
| `f4a4a21` | E1 |
| `c0e376f` | E3 |
| `74fcc13` | E5 |
| `171f036` | CI : `rom-boot` lance 30 tests ignorés de `run_cli`, pas 28 |
| `e86f457` | ligne de reset qui nomme le réveil (prérequis de E4) |
| `5712b44` | E2, E4 |
| `78d8227` | H1 à H7, H9 (partie hôte) |
| `870286a` | H4 (partie application), H8, H9 |

- **E1** : une conversion du SAR ADC qui finit au cycle où une entrée de la carte agit voyait l'ancienne tension en
  direct et la nouvelle au rejeu (ordre des événements selon leur armement). `IoMatrix::millivolts` et `spi_transfer`
  appliquent d'abord les entrées dues (`board.apply_due`).
- **E2** : `x4_panel_buttons` attend BUSY par son interruption de front descendant et un sémaphore, sans délai ; il
  écrit « 3 falls of busy by interrupt ». Le chemin `next_change` → interruption est ainsi tenu sur la machine câblée.
- **E3** : une entrée de la carte datée de `u64::MAX - 1` ou au-delà est refusée par `InputError::BeyondTime`.
- **E4** : le réveil du sommeil profond par le bouton d'allumage est enregistré puis rejoué à l'identique (test
  `run_cli` sous `--usb-power`, 7 entrées) ; la ligne de reset dit « woken from deep sleep by GPIO3 ».
- **E5** : l'ADR 0013 (point 5), la spec (2d-D12, 2d-D14, §4), la recherche et le README disent que le maintien
  automatique des pads (DIG_ISO) n'est pas modélisé et que le geste est de 1 500 ms, mesuré.
- **H1** : `Engine::finish` lit en place les changements de la carte (plus de carte dans `Finished`) ; une panique
  après la fin d'une vie ne perd plus ses écritures, le secours les reprend (`Worker.ending_card`).
- **H2** : une carte qui n'a pas pu être écrite est gardée (`keep_card`) ; l'extinction et le secours réessaient, et
  la vie suivante est refusée tant qu'elle n'est pas écrite (`card_written_before_a_life`). Remplacé par R1 de la
  seconde relecture : la vie suivante n'est plus refusée, elle lit ces changements en mémoire.
- **H3** : rallumer avec la carte déjà tenue ne bute plus sur son propre verrou (`Emulator::holds_sd_image`,
  `holds_a_file_of` de l'enveloppe : l'appareil est éteint d'abord).
- **H4** : après une extinction, le bouton d'allumage de la fenêtre rallume par l'appui de l'utilisateur, pressé à
  l'instant 0, sans le geste automatique (`Recipe::power_on_by_key`) : c'est son maintien qui fait le geste. La
  mention de l'état éteint dit « Maintenez le bouton d'allumage pour allumer ».
- **H5** : l'image de la carte n'est ouverte ou créée qu'après la ROM et les autres vérifications : un refus ne laisse
  plus d'image créée.
- **H6** : `boards::check_board_request` refuse `--sd-image`, `--usb-power` et `--power-hold` sur un appareil qui n'a
  pas la ligne, avant tout effet (CLI, `Wired::power_on`, `device::power_on` : `LaunchError::Board`, genre `board`).
- **H7** : enregistrer une carte qui ne tient pas dans une session est refusé avant la fin de la vie
  (`CommandError::NotRecorded`, genre `notRecorded`) ; l'appareil reste allumé.
- **H8** : `decodeFrame` vérifie la trame contre la dalle (taille), ses octets 5 et 7 (0 ou 1), sa génération (> 0)
  et sa région (non vide, dans l'image).
- **H9** : les bornes d'une nouvelle carte viennent de l'hôte (`DeviceView.sd_slot` : `SdSlotView`) ; le bouton
  « Allumer » grisé dit ce qui manque (`aria-describedby`), la taille hors bornes aussi.

Empreintes déplacées, avec leur raison (E2 change le firmware de référence) : `x4_panel_buttons.bin`
`82ed249a…f0b95d54` (deux constructions concordantes), `boards` `TRACE_HASH` `0x2d48_1100_f1dd_0a53`, `run_cli`
`X4_PANEL_TRACE` `88a64f66f1a1c1d4` ; nouvelle, `X4_PANEL_WAKE_TRACE` `b5891d847fa1e6f9` (E4). Aucune autre n'a bougé.

## Seconde relecture

Huit points mineurs (R1 à R8), un correctif chacun ; chaque correctif de code a un test qui échoue sans lui (vérifié en
le neutralisant puis en le rétablissant).

| Commit | Points |
|---|---|
| `34e97a4` | R1 |
| `d81dcde` | R2, R3 |
| `fc4d3f4` | R4, R7, R8 (test de l'enveloppe sur `tempfile`) |
| `df072e1` | R5, R6 |
| `2f645ab` | R8 (spec, test `spi_transfer`) |
| ce commit | note, ADR 0014 |

- **R1** : une carte que son image n'a pas prise suit la règle de la flash : ses changements restent en mémoire en
  surcouche sur l'image (`host::sd_run::CardKeeper`, `Rc<RefCell<Overlay>>` partagé avec la vie qui tourne) ; la vie
  suivante démarre dessus et les lit (Reset, touche d'allumage et Record ne sont plus bloqués) ; chaque fin de vie et
  l'extinction réessaient ; un échec durable remonte en `NotKept` au Stop et à la fermeture. Le message donne le
  remède (libérer de la place, rendre le dossier inscriptible). `CommandError::SdCard` (genre `sdCard`) nomme un refus
  de la carte : Record ne conseille alors plus « reset it ». `card_written_before_a_life` est supprimé. Tests :
  `an_unwritten_card_is_read_by_the_next_life_then_written_or_said_not_kept`,
  `the_end_of_the_next_life_writes_the_card_the_last_one_could_not`,
  `a_record_the_card_refuses_gives_the_card_s_remedy_not_a_reset`, et celui de `CardKeeper` dans `sd_run`.
- **R2** : `OpenedSd` tient l'image ouverte et sait si ce lancement l'a créée ; un refus après sa création (par
  `SdImage::open` ou par l'acceptation du fichier de flash) ferme l'image puis supprime cette image vierge, et
  seulement elle (`open_the_card_and_accept_the_flash`, `OpenedSd::refused`). Une suppression qui échoue est dite
  avec le refus (`LaunchError::BlankImageLeft`, qui garde le genre du refus) : le chemin de l'image, la cause, et que
  l'utilisateur peut l'effacer (test `a_blank_image_a_refused_start_cannot_remove_is_said_with_the_refusal`, dossier
  non inscriptible au nettoyage).
- **R3** : la CLI ouvre la carte après la ROM, comme l'application ; le contrôle qui n'ouvre aucun fichier (la carte
  n'est aucun fichier de la run, ni ses journaux) est fait avant la ROM dans les deux
  (`SdImage::refuse_the_files_of_the_run`, factorisé d'`open`). Le test CLI qui l'exigeait avant la ROM reste vrai ;
  deux tests nouveaux fixent l'ordre (CLI et application).
- **R4** : `power_on` de l'enveloppe attend la fin des extinctions en cours avant `device::power_on`
  (`after_the_power_offs`) : une carte ou une flash encore tenue par un émulateur qui s'éteint ne donne plus `InUse`.
- **R5** : en rejeu, l'appareil éteint dit « Éteint, comme dans la session : un rejeu ne peut pas rallumer
  l'appareil » (`device.offReplayed`, FR et EN).
- **R6** : le champ de taille n'est décrit par le message de ce qui manque que lorsque c'est la taille.
- **R7** : le commentaire de `power_on` (`commands.rs`) nomme la flash et la carte, et l'attente des extinctions.
- **R8** : la ligne 222 de la spec ne dit plus que SP2d donne son effet à `DIG_ISO` ; un test propre à la branche
  `spi_transfer` de E1 (`periph/io/tests.rs`) ; le test de carte de l'enveloppe passe sur `tempfile` (dépendance de
  développement ajoutée à `app/src-tauri`).

Aucune empreinte n'a bougé.

## Troisième relecture

Quatre points (N1 à N4) et un reliquat de R5, en deux commits (le code et ses tests ; la documentation).

- **N1** : quand le lancement enregistre, `device::open_sd` lit la carte telle qu'une session la garde
  (`sd_run::recorded_card`) juste après l'ouverture, avant l'acceptation du fichier de flash, comme la CLI ; une carte
  qui dépasse la borne refuse le lancement par `OpenedSd::refused`. La carte lue est gardée pour la première vie
  enregistrée (`Plan::Run.first_card`) : une grande image n'est pas lue deux fois. Test
  `a_recorded_start_with_a_card_too_full_is_refused_before_its_flash_file`.
- **N2** : ADR 0014 : « en ligne de commande » pour la sauvegarde inachevée qui empêche la carte de servir ; la
  mémoire des changements non écrits (bornée par la capacité de la carte, jusqu'à environ 2× pendant une vie), et
  pourquoi ce n'est pas le compromis de la flash ; héritier SP4 (tableau des héritiers et cartographie).
- **N3** : `rescue` écrit la flash d'abord, puis la carte.
- **N4** : quand l'image a pris la sauvegarde et que seul son journal n'a pas pu être retiré,
  `SdImageError::JournalNotRemoved` le dit (« holds what the device wrote, but its journal … could not be removed »),
  avec le remède du dossier ; `CardKeeper` n'ajoute plus « not in … yet » dans ce cas, et garde les changements en
  mémoire pour que la vie suivante n'attende pas ce journal. Tests au niveau de `CardKeeper` et du `NotKept` de
  l'extinction (point d'injection `SdImage::leave_the_journal`, propre aux tests).
- **Reliquat de R5** : une commande qui veut une vie quand un rejeu est terminé (bouton d'allumage, pas à pas…) est
  refusée par `CommandError::ReplayEnded` (genre `replayEnded`), qui dit qu'un rejeu ne prend aucune entrée, avec ses
  titres FR et EN ; tests dans `a_recorded_life_replays_identically` et `error-banner.test.tsx`.

## Vérifié en local

`CARGO_BUILD_JOBS=3`, un conteneur Docker à la fois. `cargo fmt --all` ; `tools/ci/clippy.sh … --all-targets
--all-features -D warnings` sur `boards`, `host`, `emu-cli` (et les crates touchées par chaque commit au moment du
commit) ; `cargo clippy --all-targets -D warnings` et `cargo test` de l'enveloppe Tauri (`app/src-tauri`, 16 tests) ;
`pnpm lint`, `pnpm typecheck`, Vitest (16 fichiers, 92 tests), `pnpm build`, construction release de l'enveloppe ;
bibliothèque `host` (325 tests) et tests non ignorés de `emu-cli` après la troisième relecture ;
`cargo test -p boards -p host -p session -p console-script` et les tests non ignorés de `emu-cli`, tout passe ;
`cargo check --target wasm32-unknown-unknown` de `emu-core`, `chips`, `session`, `boards` ; `cargo doc --no-deps` avec
`-D warnings` de `emu-core`, `chips`, `boards`, `session`, `host`, `soc-esp32c3`, `console-script`. Laissé à la CI : la
suite complète de l'espace de travail, la couverture.

- **Les empreintes figées n'ont pas bougé** (2d-R5) : les suites du job `reference-boot` passent en local sur les
  images de référence avec la machine câblée et le geste de 1,5 s (`soc-esp32c3` `reference_boot` 13,
  `run_cli reference_firmware` 16, `console-script` 5, `boards` `reference_signals` 1 et `reference_panel` 2), relancées
  après la relecture sur les images reconstruites ; seules les empreintes nommées plus haut ont bougé. `rom-boot` lance
  30 tests ignorés de `run_cli` (29 passent en local, le 30e, `rom_fetch_all…`, demande le dossier de ROM complet).
- **`x4_panel_buttons`** : deux constructions indépendantes dans `espressif/idf:v5.5.2` donnent les empreintes figées ;
  `build.sh` vérifie les neuf autres projets. Ses empreintes de trace, mesurées deux fois, sont les mêmes.

## Les firmwares réels, en local (tâche 17)

Fichiers de l'utilisateur, jamais en CI : `crates/host/tests/real_firmware.rs` (chemin de l'interface :
`device::power_on`, la machine sur son fil, l'écran lu par son port, capturé en PNG sous `target/real-firmware/`) et
`crates/emu-cli/tests/real_firmware.rs`. Les six passent (2026-10-15, construction release), et encore après les deux
relectures (`870286a`, `2f645ab`), avec les cinq suites de référence.

- **CrossPoint 1.6.5 atteint son écran d'accueil.** Allumé par le geste, avec une carte vierge de 1 Gio
  (`inkemu sd create`) : `[SD] SD card detected`, trois rafraîchissements (complet, partiel, partiel), puis l'écran
  d'accueil, droit dans le portrait : « No open book », « Browse Files », « Library », « File Transfer »,
  « Settings », batterie à 70 % (`target/real-firmware/crosspoint-1.6.5-blank-card.png`). Un fantôme léger de l'écran
  de démarrage reste visible, ce que font les rafraîchissements partiels du modèle. En 8 s il n'écrit aucun bloc de la
  carte. Sous `--strict-mmio` : aucun avertissement, code 0.
- **CrossPoint sans carte** : `[SD] SD card not detected`, un rafraîchissement complet, « SD card error » droit au milieu
  du portrait (pixels sombres de (174, 392) à (306, 408) ; le design l'attendait dans la moitié haute, le firmware le
  centre) (`crosspoint-1.6.5-no-card.png`).
- **Le firmware d'origine 3.0.2** : allumé par le geste de 1,5 s, il verrouille son alimentation, reste allumé, rafraîchit
  (partiel puis complet) et dessine son menu « Read », « Folder », « Settings », « Sync » (`stock-3.0.2.png`). Avec
  `--power-hold 0`, il trouve le bouton relâché, lâche son verrou, et la carte coupe l'alimentation à 0,64 s : code 0,
  sans avertissement.

## Décisions prises sans l'utilisateur

1. **Geste de 1,5 s** au lieu de 1 s (2d-D14) : mesuré sur le modèle, le firmware d'origine s'éteint quand le bouton
   est lâché à 1 000 ms et reste allumé à 1 200 ms ; 1 500 ms garde une marge (ADR 0013 point 4).
2. **Ordre des cibles de la carte** : les clés de `[wiring.button]` par ordre de nom, puis les boutons des échelles dans
   l'ordre du profil, puis `usb-power`. Le profil est dans la session : la numérotation y est stable.
3. **Le rejeu ne refait pas le geste** : ses deux entrées sont dans le journal ; la machine câblée du rejeu est allumée
   sans appui (`power_hold_ms: Some(0)`).
4. **Le bouton d'allumage du firmware de référence** : sur batterie il lâche le verrou (l'appareil s'éteint au
   relâchement), câble branché il s'endort réveillé par ce bouton. Le design (2d-D21) le voulait toujours en sommeil :
   sur batterie un X4 s'éteint par son verrou, les deux chemins de l'ADR 0013 sont ainsi tenus, chacun là où
   l'appareil le prend.
5. **Images** (2d-R10) : les rafraîchissements complets sont comparés octet par octet à l'image construite d'après la
   définition du firmware ; un rafraîchissement différentiel par son genre, ses pixels changés (un même gris sombre :
   la forme d'onde rapide de référence s'arrête avant une course entière) et tous les autres, inchangés.
6. **Le test `boards` n'enregistre pas** : l'enregistrement et le rejeu identique sont faits par le test `run_cli`
   (même firmware, script de console, format 2, même empreinte). Le script presse deux boutons ; le test `boards` presse
   les six et le bouton d'allumage.
7. **Le X3 garde la mire**, avec les boutons du X4 en attendant son câblage (SP3) ; la bannière de la mire nomme le
   sous-projet 3.
8. **Teintes** : `Shades` se sérialise `{"levels": n}` ou `"continuous"` ; une image v1 est refusée par l'interface,
   une image d'un seul niveau aussi.
9. **L'interface ne propose pas la durée du geste** : elle prend celle du profil (la CLI a `--power-hold`). Après une
   extinction, le bouton d'allumage de la fenêtre lance une nouvelle vie avec la flash courante, le bouton pressé à
   l'instant 0 et relâché quand l'utilisateur le relâche (H4) : le geste est le sien.
10. **`ButtonError::NotInSession` supprimé** (2d-D19) : un bouton pendant un enregistrement est journalisé ; un rejeu
    refuse toujours toute entrée.

## Laissé aux héritiers

| Sujet | Héritier |
|---|---|
| RAM brute du contrôleur, compteur de rafraîchissements partiels | SP4 |
| Brancher ou débrancher le câble, faire varier la batterie, insérer ou retirer la carte pendant une vie | SP4 (`cable_input`, `SdSlot::take`/`insert` existent) |
| Mémoire des changements de carte non écrits (R1) : bornée par la capacité de la carte, jusqu'à environ 2× pendant une vie ; à borner (fichier de reprise ou plafond), ADR 0014 | SP4, avec la vie de la carte |
| Durée du geste dans l'interface | quand un utilisateur la demande ; la CLI l'a |
| Câblage du X3 (UC8253, capteurs I2C) | SP3 |
| Dossier de l'hôte exposé en FAT32 virtuel | après la livraison du 2027-01-01 |
| `SEEK_DATA` pour parcourir une grande image de carte avant de l'enregistrer dans une session | à la demande (une image de 32 Gio est lue en entier aujourd'hui) |
| CrossPoint qui écrit `/.crosspoint/state.json` (tâche 17 de SP2e) : il ne l'écrit pas en 8 s sans navigation | un scénario local qui ouvre un livre ou change un réglage |
| Paramètres optiques et formes d'onde mesurés sur un appareil | SP8 (banc) ; la mention « Rendu approché » tombe d'elle-même (`is_approximation`) |

## Limites de preuve

- **L'application n'a pas été pilotée à la souris** dans une fenêtre Tauri en marche : elle est tenue par Vitest (les
  composants, le décodage des images, le dialogue), par les tests de l'enveloppe Rust et par une construction release ;
  le chemin qu'elle prend dans `host` (`device::power_on`, `Emulator`, ports, captures) est celui des tests locaux de
  CrossPoint et du firmware d'origine, qui écrivent ce que l'écran montre.
- La durée du geste est une mesure sur le modèle, pas sur un appareil.
- R2 : le refus d'`SdImage::open` juste après une création n'est pas reproductible par un test (`create_blank`
  vérifie les mêmes chemins) : le test passe par le refus du fichier de flash, qui exerce le même chemin
  (`OpenedSd::refused`).
- L'écran est une approximation (formes d'onde construites, optique supposée) ; les tests comparent au modèle, pas au
  verre d'un X4.
- CrossPoint et le firmware d'origine ne tournent qu'en local ; la CI tient le même chemin avec `x4_panel_buttons`.
- Le réveil par broche est prouvé avec le code de sommeil d'ESP-IDF 5.5.2 (`x4_panel_buttons`, câble branché,
  enregistré puis rejoué à l'identique) et par les tests de `soc-esp32c3` ; le firmware d'origine, qui s'endort réveillé par GPIO3 quand il n'est pas tenu, n'a pas
  été rejoué jusqu'à ce sommeil avec le câble.

## Cartographie

Fichiers de la cartographie du projet à mettre à jour (la mémoire n'a pas été modifiée par ce plan) :

| Fichier | Ce qui change |
|---|---|
| `architecture_overview.md` | SP2d fait : `boards::Wired` (machine câblée), entrées de la carte par la machine, `Supply` et `PowerOff`, réveil par broche, format de session 2 ; ADR 0013 |
| `backend_models.md` | `emu-core` : `BoardInput`, `BoardInputs`, `Supply` ; `soc-esp32c3` : `Wake` (`RtcTimer`, `Pins`), `Diagnostic::Reset.woken_by`, `InputError::BeyondTime` ; `boards` : `Wiring`, `Wired`, `WiredOptions`, `WiredError`, `PanelHandle`, `ButtonsHandle`, `SdHandle`, `SdStart`, `BoardRequest` ; `host::ports` : `Shades`, `PanelDescription.approximate`, `ButtonError::Refused` (sans `NotInSession`), `PowerOnError::Device` ; `host::device` : `Setup.usb_power`, `Setup.sd`/`SdSetup`, `LaunchError::SdImage`, `LaunchError::Board`, `LaunchError::BlankImageLeft` (`kind()` n'est plus `const`), `CommandError::NotRecorded`, `CommandError::SdCard` (genre `sdCard`), `CommandError::ReplayEnded` (genre `replayEnded`), `Recipe::power_on_by_key` ; `host::sd_image::SdImageError::JournalNotRemoved` ; `host::sd_run::CardKeeper` (changements de carte non écrits, en surcouche) ; `host::catalog` : `DeviceView.sd_slot` (`SdSlotView`, `SdSlotView::OF_A_DEVICE`) ; `RunEnd::PowerOff` |
| `backend_services.md` | `boards::wiring`, `boards::wired`, `boards::check_board_request` (CLI, `Wired::power_on`, `device::power_on`) ; `host::ports_wired`, `host::catalog` (`look(&DeviceProfile)`, `life_of`, `ports_of`), `host::sd_run` ; `Engine::finish` qui lit la carte en place, `Worker.card` (`CardKeeper` : la vie suivante lit les changements non écrits, chaque fin de vie et l'extinction réessaient), `Emulator::holds_sd_image` ; `device::power_on` : `SdImage::refuse_the_files_of_the_run` avant la ROM, `open_the_card_and_accept_the_flash` et `OpenedSd` (une image vierge d'un lancement refusé est supprimée ; la carte d'un lancement enregistré lue avant le fichier de flash, gardée pour la première vie dans `Plan::Run.first_card`) ; `Worker::rescue` écrit la flash puis la carte ; enveloppe : `after_the_power_offs` (un lancement attend les extinctions en cours) ; `chips::ssd1677::lines`, échelles, batterie, bouton poussoir, `SdLines` ; `IoMatrix::millivolts`/`spi_transfer` qui appliquent d'abord les entrées dues |
| `backend_controllers.md`, `api_routes.md` | `inkemu run --power-hold`, `--usb-power`, `--sd-image`, `--sd-read-only` (refusées d'abord sur un appareil sans la ligne ; l'image ouverte après la ROM, ses chemins contrôlés avant, comme dans l'application) ; ligne « the device powered off… » (code 0) ; ligne de reset « woken from deep sleep by GPIO3 » ; ligne `sd card:` du résumé ; scripts `press`/`release` ; `x4_panel_buttons` et ses tests (`boards` `reference_panel` 2, `run_cli reference_firmware` 16, `rom-boot` 30 tests ignorés de `run_cli`) ; `INKEMU_X4_PANEL_APP` ; genres d'erreur `board` et `notRecorded` de l'enveloppe |
| `database_schema.md` | session `inkemu-session 2` (entrées de carte, étiquette 4 ; carte SD de départ) ; image de carte écrite en place par son journal |
| `frontend_spa.md` | l'écran du X4 est sa dalle ; mention « Rendu approché » ; état éteint et rallumage en maintenant le bouton (en rejeu : « Éteint, comme dans la session », `device.offReplayed`) ; case câble USB ; section « Carte SD » (aucune, existante, nouvelle dans les bornes que donne l'hôte, aujourd'hui 64 à 2 097 024 Mio) ; bouton « Allumer » grisé qui dit ce qui manque ; image v2, vérifiée contre la dalle (`decodeFrame(buffer, panel)`) |
| `integrations.md` | job `reference-boot` : `INKEMU_X4_PANEL_APP`, deux suites de plus (`run_cli` 16) ; job `rom-boot` : 30 tests ignorés de `run_cli` ; `SHA256SUMS.inputs-x4-panel-buttons` (`x4_panel_buttons.bin` `82ed249a…`, BUSY par interruption) |
| `tech_debt.md` | les héritiers du tableau ci-dessus, dont la mémoire des changements de carte non écrits (SP4) |
