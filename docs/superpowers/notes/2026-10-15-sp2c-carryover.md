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

- `PanelSource { describe() -> PanelDescription ; poll(&mut self, &mut Esp32C3) -> Result<Option<PanelFrame>, FrameError> }`, une image n'étant faite que par `PanelFrame::new(FrameParts { .. })`, qui la vérifie ;
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
| pnpm 12.11.2, `minimumReleaseAge: 1440` sans aucune exception | Chaque paquet npm est la dernière version stable publiée depuis plus d'un jour au 2026-10-15 : `@tauri-apps/api` 2.12.3, `@tauri-apps/cli` 2.12.2, `@tauri-apps/plugin-dialog` 2.8.2, React 19.3.0 (les 2.12.4, 2.12.3, 2.8.3 et 19.3.1 attendent leur jour). Les crates Tauri restent 2.12.4 et 2.8.3 : crates.io n'a pas cette politique, et la CLI de Tauri n'exige que le même majeur.mineur. |
| Le dialogue de fermeture parle la langue de l'hôte (anglais) | Il vient de l'enveloppe Rust, qui ne porte pas la langue choisie ; ses lignes sont celles du journal. |
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

## Seconde relecture (72bbb40) et ses corrections

Dix-neuf constats (1 bloquant, 8 importants, 10 mineurs), tous corrigés avec un test, règles 2c-R13 (complétée) et
2c-R15 à 2c-R20 du plan :

- **B1** une mise sous tension dont le fichier de flash ne peut être pris (répertoire, verrou) perdait la flash gardée
  en mémoire : `RunFlash::begin_keeping` laisse la graine à l'appelant (`a_flash_file_that_cannot_be_taken_keeps_the_flash`).
- **I1** une panique du fil perdait la flash et laissait l'interface « allumée » : chaque passe et chaque commande sous
  `catch_unwind`, `Worker::rescue` écrit `flash_contents()`, le dit, envoie `Stopped` (`a_panic_during_a_pass_…`,
  `a_panic_during_a_command_…`).
- **I2** commandes réordonnables : file `serialized` côté interface, commande Tauri synchrone qui soumet
  (`Handle::submit`) et répond par un canal (`submitted_commands_are_done_in_their_order`, `host.test.ts`).
- **I3** mises à jour d'un ancien émulateur : démarrages numérotés, seuls les avis d'un émulateur remplacé sont gardés.
- **I4** fermeture : `CloseRequested`, `ExitRequested`, `SIGINT`/`SIGTERM` éteignent d'abord ; `Emulator::shutdown`
  rend `CommandError::NotKept`, montré dans une boîte de dialogue (`a_shutdown_that_cannot_keep_the_flash_says_why`).
- **I5** `cargo-deny` sur l'enveloppe (`app/src-tauri/deny.toml`, job `app`).
- **I6** capture : commande de l'émulateur, chemins revendiqués refusés, fichier temporaire renommé
  (`a_capture_never_replaces_a_file_of_the_run`).
- **I7** texte des consoles par lots (40 ms côté fil, une image côté interface).
- **I8** ADR renuméroté 0011.
- **M1** clés `error.maxTime`, `error.maxCycles` (et un test qui liste chaque `kind`) ; **M2** séquence ANSI coupée
  gardée par console ; **M3** boutons relâchés au `blur` ; **M4** piège à focus des dialogues ; **M5** un démarrage
  refusé laisse l'appareil allumé (sauf même fichier de flash, `Emulator::holds_flash_file`) ; **M6** rejets des
  dialogues attrapés, capture et enregistrement disent leur issue ; **M7** plus d'exception à `minimumReleaseAge` ;
  **M8** `PanelFrame::new` vérifie, `FrameError` au journal et `error.frame` dans l'interface ; **M9** mesure de
  vitesse oubliée hors exécution ; **M10** commande depuis l'observateur refusée (`CommandError::OnItsThread`).

## Laissé aux héritiers

| Report | Héritier |
|---|---|
| Brancher le SSD1677 et l'échelle de boutons dans `catalog::ports_for` ; trancher la rotation et l'ordre des touches ; mettre les boutons dans le journal de la machine (format de session 2) puis passer `reaches_firmware` à vrai sans refus d'enregistrement | SP2d |
| Réveil par GPIO3 depuis un sommeil profond (le moteur reprend déjà après toute entrée acceptée) | SP2b / SP2d |
| Scripts de console depuis l'interface, `inspect`, profils de l'utilisateur, installation d'une application en gardant NVS | SP2 suite / SP4 |
| Vue technique, moniteur de ressources, débogueur | SP4 |
| Le moteur sans E/S dans un Web Worker (séparer `engine` et `ports` des modules qui font des E/S, garde `clippy.toml` et étape WebAssembly) | SP6 |
| Empaquetage (`.deb`, `.AppImage`, `.dmg`, `.msi`), signature, notarisation, avis des bibliothèques Rust et npm liées dans « À propos », dont les avis MPL-2.0 (cssparser, cssparser-macros, selectors, dtoa-short, option-ext) et l'endroit de leur source | SP9 |
| `RUSTSEC-2024-0370` (proc-macro-error non maintenu, à la construction de glib-macros et gtk3-macros 0.18) ignoré avec sa raison dans `app/src-tauri/deny.toml` : à retirer quand une version de Tauri quitte les liaisons GTK 3. `RUSTSEC-2024-0429` (`VariantStrIter` de glib 0.18) n'est pas levé par cargo-deny et rien ne l'appelle ; les avis GTK 3 2024-0411 à 0420 ont été retirés le 2026-08-14 | SP9 (suit Tauri) |
| Traduire le dialogue de fermeture (l'enveloppe ne connaît pas la langue de l'interface) | SP2 suite |
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

Après la seconde relecture (même poste, 2026-10-15) : `cargo fmt --all --check` ; clippy `-D warnings` et rustdoc
`-D warnings` sur `host` et `emu-cli` ; `cargo test -p host -p emu-cli` : `host` 259 tests, `emu-cli` inchangé
(8, 5, `inspect_cli` 7, `licenses_cli` 2, `real_firmware` 1, `riscv_tests` 6, `run_cli` 62 et 42 ignorés, `rvtest_cli`
9) ; les 2 tests `host/tests/real_rom.rs` avec la vraie ROM : passés ; le front : Biome, `tsc`, Vitest 66 tests (deux
passes complètes), `vite build` ; l'enveloppe : fmt, clippy `-D warnings`, `cargo test` (4), `cargo build`,
`cargo deny check` (avis, interdits, licences, sources : ok) ; la fenêtre lancée puis arrêtée par `SIGTERM` sort en
130 (le gestionnaire a éteint puis quitté ; sans lui, 143).

**Non fait ici, laissé à la CI** : `cargo test --workspace`, la couverture, les autres suites ignorées, `cargo deny`
de l'espace de travail racine, l'étape WebAssembly, le job `app` lui-même. **Non vérifié du tout** : l'interface dans une vraie session utilisateur
(clics, rendu de la mire, capture) — aucun outil de pilotage de WebKitGTK n'est en place ; le bundle Tauri n'a pas été
construit (`bundle.active` est faux jusqu'à SP9).

## Cartographie

À reporter dans la cartographie de l'utilisateur (`~/.claude/projects/-home-rony-projects-xteink-emulator/memory/`),
que cet agent n'a pas modifiée :

| Fichier | À mettre à jour |
|---|---|
| `architecture_overview.md` | Nouvelle crate `host` (politique d'hôte partagée CLI/interface, membres de l'espace de travail) ; `app/` (React + Tauri, espace de travail Cargo séparé `app/src-tauri`) ; modèle à fil (`Esp32C3` non `Send`, machine construite sur le fil par `Recipe`) ; ADR 0011 ; ports `PanelSource`/`ButtonPort`. |
| `backend_services.md` | Les dix modules déplacés de `crates/emu-cli/src/` vers `crates/host/src/` (mêmes noms, réexportés par `emu_cli`) ; `host::launch` (profil, console montrée, en-tête, note de flash, flash de départ, refus de chemins, ROM) ; `engine`, `device`, `ports`, `pattern`, `catalog`, `utf8`, `capture` ; `run.rs` : `say_why`, `say_where_the_run_stopped`, `deep_sleep_line`, `journal_prefix`, `say_an_incomplete_journal`, `SLICE_CYCLES` désormais publics ; `RunEnd` sérialisable ; `launch::{claimed_paths, rom_claim}`, `session_file::{check_output_path, write_through_temporary}`, `capture::save`, `flash_state::RunFlash::{begin_keeping, save_content}` ; dans `device` : `ConsoleBatch` (40 ms), `Worker::{rescue, tell, flush_console, capture, claimed}`, `Emulator::holds_flash_file`, `Emulator::shutdown -> Result`. |
| `backend_models.md` | Types de `host` : `launch::{FlashRequest, RunRequest, ConsoleOrigin, Starting, StartError, RecordPathError}`, `ports::{Rotation, PanelOrigin, PanelDescription, RefreshKind, Region, PanelFrame, Edge, ButtonSpec, ButtonError, Life, PowerOnError, PanelSource, ButtonPort, Recipe}`, `catalog::{Look, DeviceView, ProfileRecipe, X4_BUTTONS, X3_BUTTONS}`, `engine::{Engine, Limits, Mode, Halt, State, Level, Notice, Output, EngineError, MachineStatus, DeepSleepStatus, LifeEnd}`, `device::{Speed, Setup, LaunchError, Recording, FlashKeeper, Launch, Command, CommandError, RunState, Status, VerdictView, Update, Observer, Handle, Pending, Emulator}` ; `ports::{FrameParts, FrameError}` (`PanelFrame` à champs privés, `PanelFrame::new`) ; `capture::CaptureError` ; `session_file::OutputRefusal` ; `Command::Capture`, `CommandError::{OnItsThread, Capture, NotKept}`. |
| `backend_controllers.md` | `main.rs` d'`emu-cli` appelle `host::launch` (plus de `profile_for`, `FlashOptions`, `RunRequest`, `ConsoleOrigin`, `shown_console`, `console_note`, `file_name`, `flash_note`, `load_rom`, `Starting`, `starting_flash`, `refuse_*`, `header` dans le binaire) ; sorties et codes inchangés. Nouveaux points d'entrée : les 6 commandes Tauri de `app/src-tauri/src/commands.rs` (`devices`, `power_on`, `open_replay`, `command` — synchrone, sa réponse par un canal, la capture en fait partie —, `frame`, `notices`) et leurs `kind` d'erreur (`profile`, `recordPath`, `flashFile`, `flashImage`, `firmware`, `rom`, `session`, `replay`, `powerOn`, `stopped`, `noLife`, `replaying`, `ended`, `nothingToStep`, `input`, `button`, `replayNotRecorded`, `notRecordable`, `onItsThread`, `notKept`, `ipc`, `maxTime`, `maxCycles`, `noImage`, `capture`, et `frame` côté interface) ; `lib.rs` : fermeture, sortie et signaux éteignent d'abord. |
| `api_routes.md` | Scripts pnpm de `app/package.json` (`dev`, `build`, `typecheck`, `lint`, `test`, `check`, `tauri`) ; `pnpm tauri dev` ; job CI `app` (dont `cargo-deny` sur `app/src-tauri/Cargo.toml`) ; `rom-boot` : `run-ignored-tests.sh 2 --release -p host --test real_rom`. |
| `frontend_spa.md` | L'interface existe : `app/src/` (`app.tsx`, `components/{toolbar, device-view, panel-canvas, status-panel, console-panel, journal-panel, start-dialog, about-dialog, error-banner, modal, labels}`, `ipc/{types, host, frame}`, `state/{reducer, console, batch}`, `panel/{render, refresh}`, `i18n/{fr, en, i18n, i18n-provider}`, `keyboard.ts`, `theme.ts`, `use-theme.ts`) ; démarrages numérotés, mises à jour par image, file de commandes `serialized` ; 66 tests Vitest ; ce que SP2c couvre des besoins listés (console interactive, hôte USB, deux consoles + console du profil d'abord, sommeil profond affiché, enregistrer/rejouer, composer une flash, bootloader livré, À propos) et ce qui reste (scripts, profils, OTA). |
| `integrations.md` | Crates Tauri 2.12.4 / tauri-build 2.7.3 / tauri-plugin-dialog 2.8.3, `ctrlc` 3.5 (`termination`) dans l'enveloppe ; npm `@tauri-apps/api` 2.12.3, `@tauri-apps/cli` 2.12.2, `@tauri-apps/plugin-dialog` 2.8.2, React 19.3.0, Vite 8.3.4, TypeScript 7.0.2, Vitest 5.0.4, jsdom 30.1.2, Testing Library, Biome 2.5.15, pnpm 12.11.2 (corepack ; `minimumReleaseAge: 1440` sans exception) ; `app/src-tauri/deny.toml` (MPL-2.0 admise, RUSTSEC-2024-0370 ignoré avec sa raison) ; paquets apt de Tauri en CI ; `serde_json` en dépendance de test de `host`. |
| `database_schema.md` | Pas de base ; nouveaux fichiers écrits par l'interface : session `.inkrec` à chaque fin d'une vie enregistrée, fichier de flash à chaque fin de vie, capture PNG au chemin choisi (jamais un fichier de l'exécution, par un fichier temporaire renommé) ; `localStorage` : `inkemu.language`, `inkemu.theme`. |
| `tech_debt.md` | Les reports du tableau « Laissé aux héritiers » ; les deux hypothèses de `catalog.rs` ; l'avis RUSTSEC-2024-0370 ignoré dans `app/src-tauri/deny.toml` ; les avis MPL-2.0 à livrer (SP9) ; le dialogue de fermeture non traduit. |
| `MEMORY.md` | Ligne d'état : SP2c fait sur sa branche (non mergé, non poussé), ADR 0011. |
