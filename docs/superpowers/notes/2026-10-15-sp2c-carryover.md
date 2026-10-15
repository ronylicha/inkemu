# SP2c — Note de passation

Plan : `docs/superpowers/plans/2026-10-15-sp2c-app.md` (2c-R1–2c-R10). Spec :
`docs/superpowers/specs/2026-10-15-sp2c-app-design.md` (C1–C18). Recherche : `docs/research/sp2c-app-stack.md`.
ADR 0011 (nouvel ; le numéro 0010 revient à l'ADR de SP1b-4f, mergé avant). Branche `worktree-agent-ae1327c5f7394d5b2`, partie de `main` `194f286`, ni mergée ni poussée.

## Résultat

- **`crates/host`** : les dix modules de politique d'`emu-cli` déplacés tels quels (`git mv`, `term`, `exit`,
  `flash_file`, `flash_state`, `firmware_file`, `script_file`, `session_file`, `record`, `run`, `rom_store`), et ce que
  `main.rs` décidait (`launch`). `emu-cli` les réexporte : ses tests d'intégration n'ont pas changé d'une ligne.
  Nouveaux modules : `ports` (contrat de l'écran et des boutons), `pattern` (mire de test), `catalog` (ports et
  apparence de chaque appareil, liste des appareils, recette), `utf8`, `capture` (PNG), `engine` (une vie sans E/S),
  `device` (le fil, ses commandes, la vitesse, les vies, le fichier de flash, les sessions, `power_on`,
  `open_replay`, `Handle`).
- **`app/`** : interface React 19.3 + Vite 8.3 + TypeScript 7.0 strict + Biome 2.5 + Vitest 5 (47 tests), FR/EN,
  thèmes, ARIA, clavier ; enveloppe Tauri 2.12 dans `app/src-tauri` (espace de travail Cargo à part, `Cargo.lock`
  versionné, 3 tests), commandes `devices`, `power_on`, `open_replay`, `command`, `frame`, `save_capture`,
  `notices`, un `Channel<Update>` par émulateur, l'image en `Response` brute, capacités `core:default`,
  `dialog:allow-open`, `dialog:allow-save`, CSP stricte.
- **CI** : job `app` (Biome, `tsc`, Vitest, `vite build`, puis `cargo fmt/clippy/test/build --locked` de l'enveloppe
  après les paquets système de Tauri) ; `rom-boot` lance aussi les 2 tests ignorés de `host` (`real_rom`).

## Contrat des ports (pour SP2d)

Défini dans `crates/host/src/ports.rs`, détaillé au §4 de la spec :

- `PanelSource { describe() -> PanelDescription ; poll(&mut self, &mut Esp32C3) -> Option<PanelFrame> }` ;
- `ButtonPort { buttons() -> &[ButtonSpec] ; reaches_firmware() -> bool ; set(&mut self, &mut Esp32C3, id, pressed) -> Result<(), ButtonError> }` ;
- `Recipe: Send { power_on(&mut self, Option<Vec<u8>>, recorded: bool) -> Result<Life, PowerOnError> }`, `Life { machine, panel, buttons }`.

Les ports naissent avec la machine sur le fil de l'émulateur et n'ont pas à être `Send`. `poll` rend la dernière
image terminée (génération strictement croissante depuis 1 sur une vie ; `Full` = flash noir/blanc, `Partial` sans
flash ; un octet par pixel, 0 noir, 255 blanc, niveaux également espacés). Un port qui atteint le firmware dit
`reaches_firmware() == true` et ses appuis sont refusés pendant un enregistrement (`NotInSession`) tant que le format
de session ne les tient pas. **Où brancher** : `host::catalog::ports_for` (et `catalog::look` pour les dimensions,
la rotation et la liste des boutons). L'interface lit `PanelOrigin::Device` et retire alors son bandeau de mire.

## Décisions prises sans l'utilisateur

| Décision | Raison |
|---|---|
| React plutôt qu'un TypeScript sans cadre | Pile habituelle de l'utilisateur ; l'interface grandira (moniteur, vue technique, débogueur). |
| TypeScript 7.0.2 et Biome au lieu d'ESLint | `typescript-eslint` 8.71 exige `typescript < 6.1` ; garder TS 6 démarrerait sur un compilateur plus ancien que le stable. |
| pnpm 12.11.2, `minimumReleaseAge: 1440` gardé, exceptions par version exacte | Tauri 2.12 et React 19.3.1 ont moins d'un jour ; la politique reste active pour tout le reste. Les exceptions peuvent être retirées dès le 2026-10-16. |
| Noms de fichiers du front en kebab-case | Un contrôle du poste de l'utilisateur l'exige pour les fichiers TypeScript (`device-view.tsx`). |
| Reset = `CHIP_EN` : nouvelle mise sous tension avec la flash courante | Le bouton reset du X4 est sur `CHIP_EN` (recherche matérielle) ; une session couvre une mise sous tension. |
| `Idle` et sommeil profond sans réveil = attente, pas fin, hors rejeu | Une entrée de l'utilisateur peut encore venir. |
| Enregistrer redémarre l'appareil ; éteindre écrit la session | Une session commence à la mise sous tension (ADR 0007) ; finir une session sans finir la vie fausserait ses condensats de console (`flush_console` au rejeu). |
| Rotation des panneaux X4 et X3 : 90° (hypothèse), ordre des quatre touches avant du X4 : hypothèse | Ni l'une ni l'autre n'est établie ; SP2d tranche sur le premier écran de démarrage de CrossPoint et une photo d'appareil. Marqués `ASSUMPTION:` dans `catalog.rs`. |
| La ROM se télécharge au premier usage, comme `inkemu run` | Une seule politique ; l'avis de téléchargement devient un avis du journal. |
| Les textes des erreurs restent ceux de la CLI ; l'interface titre par `kind` | Changer les textes changerait la CLI. |

## Relecture faite pendant le travail

Un agent de relecture (bogues seulement) a lu `engine.rs`, `device.rs` et `commands.rs` : aucun bogue à haute
confiance ; trois défauts de chemins d'échec, tous corrigés avec un test : (1) une mise sous tension refusée perdait
la flash (`FlashKeeper::abandon`, test `a_failed_power_on_keeps_the_flash_for_the_next`, avec et sans fichier) ;
(2) un fichier de flash impossible à écrire perdait la flash de la vie (le contenu reste en mémoire et remplace le
fichier à la fin de la vie suivante, test `a_failed_save_keeps_the_flash_for_the_next_life`) ; (3) deux démarrages
simultanés arrêtaient l'ancien émulateur sous le verrou (`Shell::keep` rend l'ancien, arrêté hors du verrou).

## Laissé aux héritiers

| Report | Héritier |
|---|---|
| Brancher le SSD1677 et l'échelle de boutons dans `catalog::ports_for` ; trancher la rotation et l'ordre des touches ; mettre les boutons dans le journal de la machine (format de session 2) puis passer `reaches_firmware` à vrai sans refus d'enregistrement | SP2d |
| Réveil par GPIO3 depuis un sommeil profond (le moteur reprend déjà après toute entrée acceptée) | SP2b / SP2d |
| Scripts de console depuis l'interface, `inspect`, profils de l'utilisateur, installation d'une application en gardant NVS | SP2 suite / SP4 |
| Vue technique, moniteur de ressources, débogueur | SP4 |
| Le moteur sans E/S dans un Web Worker (séparer `engine` et `ports` des modules qui font des E/S, garde `clippy.toml` et étape WebAssembly) | SP6 |
| Empaquetage (`.deb`, `.AppImage`, `.dmg`, `.msi`), signature, notarisation, avis des bibliothèques Rust et npm liées dans « À propos » | SP9 |
| Le temps d'appareil ne s'écoule pas pendant une attente (`Idle`) : une entrée tapée après dix secondes murales est datée à l'instant de l'arrêt | Documenté (C5) ; à revoir si un appareil a besoin d'un temps qui court sans rien de planifié |

## Limites de preuve

Sur le poste de développement (partagé avec d'autres agents, charge moyenne 12 à 33 sur 16 cœurs,
`CARGO_BUILD_JOBS=3`) :

- `cargo fmt` ; `cargo clippy --all-targets -- -D warnings` sur `host` et `emu-cli` ; rustdoc `-D warnings` sur
  `host` et `emu-cli` ;
- `cargo test -p host -p emu-cli` : `host` 247 tests (192 déplacés, 55 nouveaux), `emu-cli` inchangé (bibliothèque 8,
  binaire 5 — les 5 autres sont passés dans `host::launch` —, `run_cli` 62 et 42 ignorés, `inspect_cli` 7, `licenses_cli`
  2, `real_firmware` 1, `riscv_tests` 6, `rvtest_cli` 9) ; avant le déplacement : 200 tests de bibliothèque, tous
  repassés à l'identique ;
- les 2 tests ignorés `host/tests/real_rom.rs` avec la vraie ROM (`INKEMU_ROM_ELF_DIR`) : `power_on` démarre la ROM sur
  son fil, la bannière arrive sur la console du profil, le fichier de flash de 16 Mio est écrit ; une vie enregistrée
  depuis l'interface se rejoue identique par `open_replay` ;
- les 28 tests ignorés de la vraie ROM de `run_cli` (hors `reference_firmware`, ceux du job `rom-boot`), en debug,
  le 2026-10-15 : 27 passés avec le cache de ROM du poste, le 28e (`rom_fetch_all_…`) échoué faute du fichier
  `esp32c3_rev101_rom.elf` dans ce cache (cause d'environnement), puis passé avec un répertoire rempli par
  `inkemu rom fetch --all` de cette branche (téléchargement réel de l'archive épinglée, vérifiée) : 28 sur 28 ;
- le front : Biome, `tsc` (TypeScript 7), Vitest 47 tests, `vite build` ;
- l'enveloppe : `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, `cargo test` (3), `cargo build`
  (debug) ; lancement réel de la fenêtre pendant 8 s sans erreur (arrêtée par `timeout`).

**Non fait ici, laissé à la CI** : `cargo test --workspace`, la couverture, les autres suites ignorées, `cargo deny`,
l'étape WebAssembly, le job `app` lui-même. **Non vérifié du tout** : l'interface dans une vraie session utilisateur
(clics, rendu de la mire, capture) — aucun outil de pilotage de WebKitGTK n'est en place ; le bundle Tauri n'a pas été
construit (`bundle.active` est faux jusqu'à SP9).

## Cartographie

À reporter dans la cartographie de l'utilisateur (`~/.claude/projects/-home-rony-projects-xteink-emulator/memory/`),
que cet agent n'a pas modifiée :

| Fichier | À mettre à jour |
|---|---|
| `architecture_overview.md` | Nouvelle crate `host` (politique d'hôte partagée CLI/interface, membres de l'espace de travail) ; `app/` (React + Tauri, espace de travail Cargo séparé `app/src-tauri`) ; modèle à fil (`Esp32C3` non `Send`, machine construite sur le fil par `Recipe`) ; ADR 0011 ; ports `PanelSource`/`ButtonPort`. |
| `backend_services.md` | Les dix modules déplacés de `crates/emu-cli/src/` vers `crates/host/src/` (mêmes noms, réexportés par `emu_cli`) ; `host::launch` (profil, console montrée, en-tête, note de flash, flash de départ, refus de chemins, ROM) ; `engine`, `device`, `ports`, `pattern`, `catalog`, `utf8`, `capture` ; `run.rs` : `say_why`, `say_where_the_run_stopped`, `deep_sleep_line`, `journal_prefix`, `say_an_incomplete_journal`, `SLICE_CYCLES` désormais publics ; `RunEnd` sérialisable. |
| `backend_models.md` | Types de `host` : `launch::{FlashRequest, RunRequest, ConsoleOrigin, Starting, StartError, RecordPathError}`, `ports::{Rotation, PanelOrigin, PanelDescription, RefreshKind, Region, PanelFrame, Edge, ButtonSpec, ButtonError, Life, PowerOnError, PanelSource, ButtonPort, Recipe}`, `catalog::{Look, DeviceView, ProfileRecipe, X4_BUTTONS, X3_BUTTONS}`, `engine::{Engine, Limits, Mode, Halt, State, Level, Notice, Output, EngineError, MachineStatus, DeepSleepStatus, LifeEnd}`, `device::{Speed, Setup, LaunchError, Recording, FlashKeeper, Launch, Command, CommandError, RunState, Status, VerdictView, Update, Observer, Handle, Emulator}`. |
| `backend_controllers.md` | `main.rs` d'`emu-cli` appelle `host::launch` (plus de `profile_for`, `FlashOptions`, `RunRequest`, `ConsoleOrigin`, `shown_console`, `console_note`, `file_name`, `flash_note`, `load_rom`, `Starting`, `starting_flash`, `refuse_*`, `header` dans le binaire) ; sorties et codes inchangés. Nouveaux points d'entrée : les 7 commandes Tauri de `app/src-tauri/src/commands.rs` et leurs `kind` d'erreur (`profile`, `recordPath`, `flashFile`, `flashImage`, `firmware`, `rom`, `session`, `replay`, `powerOn`, `stopped`, `noLife`, `replaying`, `ended`, `nothingToStep`, `input`, `button`, `replayNotRecorded`, `notRecordable`, `ipc`, `maxTime`, `maxCycles`, `noImage`, `capture`). |
| `api_routes.md` | Scripts pnpm de `app/package.json` (`dev`, `build`, `typecheck`, `lint`, `test`, `check`, `tauri`) ; `pnpm tauri dev` ; job CI `app` ; `rom-boot` : `run-ignored-tests.sh 2 --release -p host --test real_rom`. |
| `frontend_spa.md` | L'interface existe : `app/src/` (`app.tsx`, `components/{toolbar, device-view, panel-canvas, status-panel, console-panel, journal-panel, start-dialog, about-dialog, error-banner, modal, labels}`, `ipc/{types, host, frame}`, `state/{reducer, console}`, `panel/{render, refresh}`, `i18n/{fr, en, i18n, i18n-provider}`, `keyboard.ts`, `theme.ts`, `use-theme.ts`) ; 47 tests Vitest ; ce que SP2c couvre des besoins listés (console interactive, hôte USB, deux consoles + console du profil d'abord, sommeil profond affiché, enregistrer/rejouer, composer une flash, bootloader livré, À propos) et ce qui reste (scripts, profils, OTA). |
| `integrations.md` | Tauri 2.12.4 / tauri-build 2.7.3 / tauri-plugin-dialog 2.8.3, `@tauri-apps/cli` 2.12.3, React 19.3.1, Vite 8.3.4, TypeScript 7.0.2, Vitest 5.0.4, jsdom 30.1.2, Testing Library, Biome 2.5.15, pnpm 12.11.2 (corepack ; `minimumReleaseAge` et ses exceptions) ; paquets apt de Tauri en CI ; `serde_json` en dépendance de test de `host`. |
| `database_schema.md` | Pas de base ; nouveaux fichiers écrits par l'interface : session `.inkrec` à chaque fin d'une vie enregistrée, fichier de flash à chaque fin de vie, capture PNG au chemin choisi ; `localStorage` : `inkemu.language`, `inkemu.theme`. |
| `tech_debt.md` | Les reports du tableau « Laissé aux héritiers » ; les deux hypothèses de `catalog.rs` ; les exceptions `minimumReleaseAgeExclude` à retirer après le 2026-10-16. |
| `MEMORY.md` | Ligne d'état : SP2c fait sur sa branche (non mergé, non poussé), ADR 0011. |
