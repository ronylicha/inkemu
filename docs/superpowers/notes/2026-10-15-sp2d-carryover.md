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

## Vérifié en local

`CARGO_BUILD_JOBS=3`, un conteneur Docker à la fois. `cargo fmt --all` ; `tools/ci/clippy.sh … --all-targets
--all-features -D warnings` sur `boards`, `host`, `emu-cli` (et les crates touchées par chaque commit au moment du
commit) ; `cargo clippy --all-targets -D warnings` et `cargo test` de l'enveloppe Tauri (`app/src-tauri`, 14 tests) ;
`pnpm lint`, `pnpm typecheck`, Vitest (15 fichiers, 81 tests), `pnpm build`, construction release de l'enveloppe ;
`cargo test -p boards -p host -p session -p console-script` et les tests non ignorés de `emu-cli`, tout passe ;
`cargo check --target wasm32-unknown-unknown` de `emu-core`, `chips`, `session`, `boards` ; `cargo doc --no-deps` avec
`-D warnings` de `emu-core`, `chips`, `boards`, `session`, `host`, `soc-esp32c3`, `console-script`. Laissé à la CI : la
suite complète de l'espace de travail, la couverture.

- **Les empreintes figées n'ont pas bougé** (2d-R5) : les suites du job `reference-boot` passent en local sur les
  images de référence avec la machine câblée et le geste de 1,5 s (`soc-esp32c3` `reference_boot` 13,
  `run_cli reference_firmware` 15, `console-script` 5, `boards` `reference_signals` 1 et `reference_panel` 2).
- **`x4_panel_buttons`** : deux constructions indépendantes dans `espressif/idf:v5.5.2` donnent les empreintes figées ;
  `build.sh` vérifie les neuf autres projets. Ses empreintes de trace, mesurées deux fois, sont les mêmes.

## Les firmwares réels, en local (tâche 17)

Fichiers de l'utilisateur, jamais en CI : `crates/host/tests/real_firmware.rs` (chemin de l'interface :
`device::power_on`, la machine sur son fil, l'écran lu par son port, capturé en PNG sous `target/real-firmware/`) et
`crates/emu-cli/tests/real_firmware.rs`. Les cinq passent (2026-10-15, construction release).

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
   extinction, le bouton d'allumage de la fenêtre lance une nouvelle vie avec la flash courante.
10. **`ButtonError::NotInSession` supprimé** (2d-D19) : un bouton pendant un enregistrement est journalisé ; un rejeu
    refuse toujours toute entrée.

## Laissé aux héritiers

| Sujet | Héritier |
|---|---|
| RAM brute du contrôleur, compteur de rafraîchissements partiels | SP4 |
| Brancher ou débrancher le câble, faire varier la batterie, insérer ou retirer la carte pendant une vie | SP4 (`cable_input`, `SdSlot::take`/`insert` existent) |
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
- L'écran est une approximation (formes d'onde construites, optique supposée) ; les tests comparent au modèle, pas au
  verre d'un X4.
- CrossPoint et le firmware d'origine ne tournent qu'en local ; la CI tient le même chemin avec `x4_panel_buttons`.
- Le réveil par broche est prouvé avec le code de sommeil d'ESP-IDF 5.5.2 (`x4_panel_buttons`, câble branché) et par
  les tests de `soc-esp32c3` ; le firmware d'origine, qui s'endort réveillé par GPIO3 quand il n'est pas tenu, n'a pas
  été rejoué jusqu'à ce sommeil avec le câble.

## Cartographie

Fichiers de la cartographie du projet à mettre à jour (la mémoire n'a pas été modifiée par ce plan) :

| Fichier | Ce qui change |
|---|---|
| `architecture_overview.md` | SP2d fait : `boards::Wired` (machine câblée), entrées de la carte par la machine, `Supply` et `PowerOff`, réveil par broche, format de session 2 ; ADR 0013 |
| `backend_models.md` | `emu-core` : `BoardInput`, `BoardInputs`, `Supply` ; `boards` : `Wiring`, `Wired`, `WiredOptions`, `WiredError`, `PanelHandle`, `ButtonsHandle`, `SdHandle`, `SdStart` ; `host::ports` : `Shades`, `PanelDescription.approximate`, `ButtonError::Refused` (sans `NotInSession`), `PowerOnError::Device` ; `host::device` : `Setup.usb_power`, `Setup.sd`/`SdSetup`, `LaunchError::SdImage` ; `RunEnd::PowerOff` |
| `backend_services.md` | `boards::wiring`, `boards::wired` ; `host::ports_wired`, `host::catalog` (`look(&DeviceProfile)`, `life_of`, `ports_of`), `host::sd_run` ; `chips::ssd1677::lines`, échelles, batterie, bouton poussoir, `SdLines` |
| `backend_controllers.md`, `api_routes.md` | `inkemu run --power-hold`, `--usb-power`, `--sd-image`, `--sd-read-only` ; ligne « the device powered off… » (code 0) ; ligne `sd card:` du résumé ; scripts `press`/`release` ; `x4_panel_buttons` et ses tests (`boards` `reference_panel` 2, `run_cli reference_firmware` 15) ; `INKEMU_X4_PANEL_APP` |
| `database_schema.md` | session `inkemu-session 2` (entrées de carte, étiquette 4 ; carte SD de départ) ; image de carte écrite en place par son journal |
| `frontend_spa.md` | l'écran du X4 est sa dalle ; mention « Rendu approché » ; état éteint et rallumage par le bouton ; case câble USB ; section « Carte SD » (aucune, existante, nouvelle 64 à 2 097 024 Mio) ; image v2 |
| `integrations.md` | job `reference-boot` : `INKEMU_X4_PANEL_APP`, deux suites de plus ; `SHA256SUMS.inputs-x4-panel-buttons` |
| `tech_debt.md` | les héritiers du tableau ci-dessus |
