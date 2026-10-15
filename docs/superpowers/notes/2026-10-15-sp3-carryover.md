# SP3 — Note de passation

Branche de SP3, partie de `main` à `6691a7d` (SP2 terminé). PR #22 ouverte ; les corrections de sa relecture sont
sur la branche, non poussées par l'architecte. Spec :
`docs/superpowers/specs/2026-10-15-sp3-x3-design.md` (décisions 3-D1 à 3-D17) ; plan :
`docs/superpowers/plans/2026-10-15-sp3-x3.md` (règles 3-R1 à 3-R8, tâches 1 à 7) ; recherche :
`docs/research/sp3-x3-firmware.md` et `docs/research/sp3-x3-chips.md` ; ADR 0015.

## Résultat

Le X3 est émulé et montré dans l'application de bureau comme le X4 : I2C0 atteint les trois puces de son bus par la
matrice GPIO, son écran est sa dalle derrière un UC8253, ses boutons, son bouton d'allumage et son lecteur de carte à
alimentation commutée sont sur les broches de son profil. CrossPoint 1.6.5 le reconnaît et, au second démarrage,
dessine son écran d'accueil.

| Commit | Contenu |
|---|---|
| `fb8bb2f` | recherche : CrossPoint 1.6.5 et FreeInk sur le X3 ; UC8253/UC8279d, BQ27220, DS3231, QMI8658 |
| `93f9934` | `emu-core` : `I2cTarget` ; `soc-esp32c3` : I2C0 vers les puces par la matrice, condition par condition |
| `9122e5a` | `chips` : verre partagé, lignes génériques, UC8253, les trois puces I2C, alimentation de la carte commutée |
| `5ef1290` | `boards`, `session` : le X3 câblé par son profil ; le rejeu garde la poignée I2C |
| `6093294` | `host`, `app` : le X3 au catalogue et dans les ports ; CrossPoint sur le X3 en test local |
| `c319996` | `chips` : flux pseudo-aléatoires sans panique (UC8253, puces I2C) ; copie `N2OCP` par octets |
| `7f5140e` | firmware de référence `x3_panel_sensors`, son test `reference_x3`, le job CI `reference-boot` |
| `7003ee5` | spec, plan, ADR 0015, README (EN et FR), cette note |
| `d3362c4`, `5ec2ba8` | analyse de cause racine du premier démarrage de CrossPoint : test local, recherche, documents |
| `7d41f9f`, `0aa98d6` | relecture de la PR #22 : tirages de la carte, chemins d'I2C0 alignés, bit `SPI_BE`, `signals.rs` |
| (suivant) | relecture de la PR #22 : documents |

## Vérifié en local

- `cargo fmt --all --check` ; `cargo clippy --all-targets -- -D warnings` sur `emu-core`, `soc-esp32c3`, `chips`,
  `boards`, `session`, `host`, `emu-cli` ; `cargo doc --no-deps` sans avertissement sur `emu-core`, `chips`, `boards`,
  `soc-esp32c3`, `session`, `host` ; `cargo check --target wasm32-unknown-unknown` sur `emu-core`, `chips`, `session`,
  `boards` ; `cargo check --workspace --tests` et la vérification de Tauri.
- Tests étroits : `chips` (dont l'UC8253 et les puces I2C), `boards`, `session`, `emu-core`, la bibliothèque de
  `soc-esp32c3` (729), celle de `host` (325), `run_cli` (65).
- Le firmware de référence construit deux fois dans `espressif/idf:v5.5.2`, mêmes empreintes
  (`x3_panel_sensors.bin` `63862c73…`) ; son bootloader et sa table sont ceux des autres projets ; les autres projets
  gardent leurs empreintes épinglées et les images fusionnées passent `SHA256SUMS.output`.
- `reference_x3` sur ce firmware, en release : passe, 7 s ; toutes ses assertions passaient au premier essai, sauf
  l'empreinte de trace, mesurée deux fois identique (`0xaa63_f902_ff80_c010`) puis épinglée ; la lecture de l'horloge
  dans la seconde vie, mesurée puis épinglée exactement (`12:34:59`, l'heure réglée à `12:34:56` plus le temps écoulé).
- Les suites `reference_panel` (2) et `reference_signals` (1) du X4 et `reference_boot` (13) de `soc-esp32c3`
  passent sur les images de cette construction : leurs empreintes de trace n'ont pas bougé (3-D3).
- CrossPoint 1.6.5 sur le X3, par `host` (test local, capture `target/real-firmware/crosspoint-1.6.5-x3.png`) et par la
  CLI (`inkemu run --device x3`, deux démarrages, fichier de flash et carte vierge).

## Le firmware de référence

`x3_panel_sensors` (Apache-2.0) : alimente la carte, lit la jauge, règle et lit l'horloge, configure et lit le
capteur, sonde 0x6A, dessine un rafraîchissement complet (cadre, carré, barre de 4 pixels par pourcent) puis un
rafraîchissement partiel (un second carré), attend `BUSY` par interruption, dort jusqu'à son bouton. Son test lit la
console ligne à ligne, compare les deux images octet par octet, vérifie l'état des puces, le réveil par GPIO seul, une
seconde vie après un appui où l'horloge compte toujours depuis l'heure réglée, aucun diagnostic, et l'empreinte de
trace.

## Décisions prises sans l'utilisateur

1. **Le niveau des conditions** pour l'I2C (3-D1), pas celui du bit.
2. **Deux chemins dans I2C0** (3-D3) : sans puce, celui d'avant SP3, pour qu'aucune empreinte ne bouge ; avec puces,
   une condition à la fois. Le coût : deux chemins à garder alignés (ADR 0015, conséquences).
3. **Le verre partagé** (`chips::glass`) et les lignes génériques, avec réexports aux anciens chemins (3-D5).
4. **L'heure du DS3231 est celle de l'appareil**, depuis un départ du profil (3-D9) ; l'heure de l'hôte attend SP4.
5. **Pas de changement du format des sessions** (3-D12).
6. **La configuration UC8253 par défaut** pour le profil du X3 : CrossPoint pilote le premier lot par défaut ;
   `xteink-x3-uc8279d` existe pour le second.
7. **Les touches de face** placées comme sur le X4 (hypothèse écrite dans le profil).
8. **CrossPoint au premier démarrage** : il se bloque pour de bon sur le mutex de `Wire`. Le coordinateur a refusé
   cette conclusion sans preuve ; l'analyse de cause racine (section suivante) l'établit : faute du firmware, le
   modèle n'a rien à corriger et n'essaie pas de la contourner (pas de NVS pré-rempli). Le test local vérifie la
   signature des deux états du mutex ; le README dit de garder la flash et de démarrer deux fois.
9. **Un flux pseudo-aléatoire par puce** ajouté après coup (règle 3-R2), qui a montré que la copie `N2OCP` pixel par
   pixel coûtait trop : elle copie désormais un octet de huit pixels à la fois.
10. **Les tirages de la carte** (3-D16, relecture X1 et X3) : un mécanisme général du SoC plutôt qu'un réglage du
    bus I2C, parce que la ligne de données de l'écran en a besoin aussi.

## Analyse de cause racine : le premier démarrage de CrossPoint sur le X3

Détail et références de ligne : `docs/research/sp3-x3-firmware.md` §7.

1. **Versions exactes.** pioarduino `55.03.311` livre Arduino-ESP32 **3.3.11** (son `platform.json`), pas 3.3.1
   comme la recherche le disait ; les journaux du binaire portent les numéros de ligne de 3.3.11 (`Wire.cpp:556` pour
   `write()`, 544 en 3.3.1), et son en-tête nomme ESP-IDF 5.5.5. Recherche corrigée.
2. **La séquence.** `runProbePass()` : `Wire.begin()` crée le mutex, `Wire.end()` libère les tampons et rend le mutex
   sans le détruire. `getWakeupReason()` → `isUsbConnected()` → `X3GPIO::readI2CReg16LE()` : `beginTransmission`
   prend le mutex (il existe) ; `endTransmission(false)` rend 4 sur le tampon nul avant son seul `xSemaphoreGive`,
   qui n'est que sur le chemin `sendStop` ; la seconde tentative ne reprend pas le mutex (même tâche). Puis
   `Imu::begin()` → `ensureWire()` → `Wire.begin()` → `xSemaphoreTake(lock, portMAX_DELAY)` sur le mutex de sa propre
   tâche : `xQueueSemaphoreTake` (IDF 5.5.1 et 5.5.5, identiques) n'a pas de test du détenteur, la tâche attend pour
   toujours. Aucun chien de garde ne la relance (la tâche inactive tourne ; la tâche `loop` d'Arduino n'est pas
   abonnée par défaut).
3. **Ce que fait le modèle sur ce chemin : rien.** I2C0 instrumenté en local (un journal de chaque accès, retiré
   ensuite, rien de commité) : le dernier accès est l'`INT_ENA` mis à zéro par le `Wire.end()` de la sonde ; aucun
   accès ne suit pendant les 2,3 s jusqu'à la fin de la course, et une course de 30 s reste figée au même point. Le
   blocage est dans l'ordonnanceur, pas dans un pilote qui attendrait une interruption. Les pistes matérielles
   (`TRANS_COMPLETE`, `NACK`, `TIME_OUT`, durées, `FSM_RST`, `CLR_BUS`, état des lignes après un NACK, adresse absente
   après une présente) ne sont pas sur ce chemin : la sonde de CrossPoint n'a fait que des lectures réussies
   (score 3/3, 0x6A jamais sondé puisque 0x6B répond).
4. **Contre-épreuve.** Le démarrage suivant ne diffère que par l'état du mutex : la sonde ne tourne pas, le mutex
   n'existe pas, `beginTransmission` journalise `could not acquire lock` et ne prend rien ; l'écran d'accueil suit.
   Les deux signatures sont vérifiées par le test local. Les verrous HAL sont compilés : la ligne
   `could not acquire lock` de `beginTransmission` n'existe que sans `CONFIG_DISABLE_HAL_LOCKS`.
5. **Sur un appareil réel.** L'amont a fusionné le 2026-10-01, après 1.6.5 (2026-09-27), « fix: Remove BQ27220
   battery current USB detection for X3 » (PR #3809), qui retire exactement cette lecture brute ; son essai sur un X3
   physique relève les mêmes lignes `could not acquire lock` et `NULL TX buffer pointer` au démarrage. Le ticket
   #3498 (2026-09-11) signale un X3 « frozen after flashing ». Un X3 mis à jour depuis un CrossPoint antérieur garde
   `cphw/dev_det` en NVS et ne prend jamais ce chemin ; par la source, le blocage n'arriverait qu'une fois, à la
   première installation, et le verdict est écrit avant lui. Ni le ticket (sans version ni journal) ni la PR (essai de
   démarrages avec le verdict en cache) ne montrent le blocage sur un appareil.
6. **Conclusion.** Faute du firmware, sans rapport avec un timing, **déduite de l'exécution sous inkemu et de la
   source, non observée sur un X3 neuf** : par la source, un vrai X3 neuf la rencontrerait aussi. Aucun changement du
   modèle pour elle. Premier démarrage de CrossPoint 1.6.5 sous inkemu : sonde 3/3, `VER=FF FF FF` (depuis 3-D16),
   verdict écrit, blocage ; second : écran d'accueil.

## Relecture de la PR #22

0 bloquant, 2 importants, 7 mineurs ; tous corrigés, chaque correction de code avec un test qui échoue sans elle
(vérifié en retirant la correction).

| # | Constat | Correction |
|---|---|---|
| X1 (I) | une ligne I2C relâchée se lisait basse sans tirage interne : ACK fantôme, `0x00` | tirages de la carte (3-D16) : `Esp32C3::pull_up_on_board`, `[wiring.i2c] pull_up` obligatoire, `true` sur le X3 (hypothèse sourcée en RC §1 point 5) ; tests I2C0 tirages internes éteints, câblage du X3, refus d'un bus sans `pull_up` |
| X2 (I) | « un vrai X3 neuf se bloque aussi » affirmé, déduit seulement | « déduit de l'exécution sous inkemu et de la source, non observé sur un X3 neuf » dans la spec, les README, cette note et la recherche (§7 et §11 point 3 concordent) |
| X3 | `VER=00 00 00` sous inkemu, `FF FF FF` relevé sur le terrain | tirage de la ligne de données de l'écran du X3 (`mosi_pull_up`) ; CrossPoint lit `FF FF FF` et garde l'UC8253 (test local) |
| X4 | chemins d'I2C0 divergents (`READ` vide, liste vide) | alignés (3-D17) ; test croisé ; empreintes du X4 et des firmwares de référence inchangées |
| X5 | doc de `signals.rs` périmée | comptes de traits corrigés |
| X6 | bit BE de CTRL1 du QMI8658 ignoré sans hypothèse | `ASSUMPTION` et `report_once` sur une lecture de sortie avec le bit mis ; test |
| X7 | §7 de la spec incomplète | hypothèses de la jauge, de l'horloge, du capteur et des tirages ajoutées |
| X8 | « counter-clockwise » dans la recherche §9 | « clockwise », justifié |
| X9 | numérotation de cette note, état de la branche | corrigés ; Cartographie laissée au coordinateur |

## Laissé aux héritiers

| Héritier | Quoi |
|---|---|
| SP4 | l'heure du DS3231 donnée par l'hôte, choix de l'utilisateur, format de session 3 |
| SP4 | une batterie qui se vide, le câble du X3 vu par le courant de la jauge |
| SP4 | les diagnostics des puces I2C et de l'UC8253 dans l'interface (aujourd'hui pris par les tests seulement) |
| SP4 | les registres d'I2C0 acceptés sans effet (`docs/registers-accepted-without-effect.md`) |
| SP8 | la géométrie réelle du verre du X3 (quel bord porte G0), sa fréquence, les durées de l'UC8253 mesurées |
| quand un firmware s'en sert | les alarmes et la broche INT du DS3231 ; les formes d'onde en gris de 49 octets de l'UC8279d |
| amont (CrossPoint) | le blocage du premier démarrage sur le mutex de `Wire`, déjà corrigé après 1.6.5 (PR #3809) : vérifier la version suivante dès son premier démarrage |

## Limites de preuve

- Les valeurs des puces sont des paramètres du profil, pas des mesures d'un appareil.
- Le sens du balayage du verre et la place des touches de face sont supposés ; une photo d'un X3 qui affiche une mire
  connue les tranche.
- CrossPoint sur le X3 n'est vérifié qu'en local (aucun binaire tiers en CI).
- Le blocage de CrossPoint 1.6.5 au premier démarrage est **déduit de l'exécution sous inkemu et de la source, non
  observé sur un X3 neuf** ; le ticket #3498 et la PR #3809 de l'amont s'accordent avec lui sans le montrer.
- Les tirages de la carte sont supposés (I2C) ou déduits d'un relevé de terrain (ligne de données de l'écran) ; leurs
  valeurs ne sont pas connues.

## Cartographie

Fichiers de la cartographie du projet à mettre à jour (la mémoire n'a pas été modifiée par ce plan) :

| Fichier | Ce qui change |
|---|---|
| `architecture_overview.md` | SP3 fait : `I2cTarget`, I2C0 vers les puces par la matrice (deux chemins), verre partagé `chips::glass`, UC8253, puces I2C, X3 câblé ; ADR 0015 |
| `backend_models.md` | `emu-core` : `I2cTarget` ; `soc-esp32c3` : `I2cWires` ; `chips` : `glass::{PanelImage, RefreshKind, Provenance, Optics, Transfer}`, `lines::PanelLines`/`PanelController`, `Uc8253`, `Uc8253Config`, `Uc8253Geometry`, `Uc8253Timing`, `Variant`, `uc8253::Diagnostic`, `Bq27220`(`Config`), `Ds3231`(`Config`), `DateTime`, `Qmi8658`(`Config`), `I2cDiagnostic`, `SdPower` ; `boards` : `Controller::Uc8253`, `PanelConfig`, `I2cWiring`, `SdWiring.power`, `PanelChip`, `I2cHandle`, `Wired.i2c`, `WiredError::DisplayUc8253`/`I2cChip` |
| `backend_services.md` | `IoMatrix::attach_i2c` et `i2c_address/write/read/stop` ; I2C0 `Running::Whole`/`Stepping` ; `Esp32C3::attach_i2c` ; `chips::uc8253` (commandes, RAM, fenêtre, `N2OCP` par octets, formes d'onde) ; `chips::i2c` ; `SdLines::switched`, `SdCard::power_cycle` ; `boards::wiring` (tables `[wiring.i2c]`) ; `host::catalog` (X3 câblé, branche de mire supprimée), `host::ports_wired` (taille par la configuration, image par la poignée) |
| `backend_controllers.md`, `api_routes.md` | `inkemu run --device x3` câblé (`--usb-power` refusé pour le X3) ; `x3_panel_sensors` et son test (`boards` `reference_x3` 1) ; `INKEMU_X3_PANEL_APP` ; test local `crosspoint_1_6_5_on_the_x3_reaches_its_home_screen_at_its_second_boot` |
| `database_schema.md` | format des sessions inchangé ; profil `x3.toml` : `[wiring]` du X3, `[power]` |
| `frontend_spa.md` | l'écran du X3 est sa dalle (plus de mire) ; pas de case câble pour le X3 ; bannière de mire sans « sous-projet 3 » |
| `integrations.md` | job `reference-boot` : `INKEMU_X3_PANEL_APP`, suite `reference_x3` ; `SHA256SUMS.inputs-x3-panel-sensors` (`x3_panel_sensors.bin` `63862c73…`) ; ESP-IDF `i2c_master` dans un firmware de référence |
| `tech_debt.md` | les héritiers du tableau ci-dessus ; les deux chemins d'I2C0 |
