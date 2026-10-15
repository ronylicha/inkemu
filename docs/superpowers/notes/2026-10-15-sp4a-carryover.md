# SP4a — Note de passation

Branche de SP4a, partie de `main` à `6691a7d` (SP2 fait). Branche distante : `sp4a-debugger` (PR #21). Découpage
de SP4 : `docs/superpowers/notes/2026-10-15-sp4-decoupage.md` ; recherche : `docs/research/sp4-debugging.md` ; spec :
`docs/superpowers/specs/2026-10-15-sp4a-debugger-design.md` (décisions 4a-D1 à 4a-D13) ; plan :
`docs/superpowers/plans/2026-10-15-sp4a-debugger.md` (règles 4a-R1 à 4a-R8, tâches 1 à 8) ; ADR 0016.

## Résultat

Le débogueur est dans l'application. Un bouton « Débogueur » de la barre ouvre un panneau qui montre :

- le processeur : PC, mode, instructions et cycles, registres x0–x31 par leurs noms ABI, CSR par leurs noms ;
- le code autour du PC, désassemblé (RV32IMC, pseudo-instructions d'objdump, cibles absolues), ligne du PC marquée ;
  de la flash, seules les pages d'où le processeur a chargé une instruction pendant cette vie, les autres dites
  « non exécutées » ;
- une page de mémoire lue sans effet, 16 octets par ligne, avec ce qui n'est pas montré et pourquoi ;
- les points d'arrêt (posés depuis une ligne de code ou une adresse) et les points de surveillance (lecture,
  écriture ou accès, 1 à 4 096 octets).

Un arrêt met l'appareil en pause et le dit : statut, annonce de la fenêtre, une ligne au journal. Les points suivent
l'utilisateur d'un lancement à l'autre, survivent aux resets et se posent pendant un rejeu. L'état où une vie a fini
reste lisible jusqu'à la suivante. Un arrêt ne change rien à l'exécution : même empreinte de trace, même temps
d'appareil, rejeu « identique ». Aucune empreinte figée n'a bougé.

| Commit | Contenu |
|---|---|
| `5875cee` | recherche, découpage de SP4 en six plans, spec et plan de SP4a |
| `a9460f4` | `cpu-rv32` : `disasm` (`ABI_NAMES`, `register_name`, `csr_name`, `disassemble`) |
| `2ced25f` | `soc-esp32c3` : `run_stopping` qui partage la boucle de `run_within` (`run_loop<S: Stops>`, `NoStops` d'arrêt `Infallible`), points d'arrêt, points de surveillance notés par le bus, `peek` sans effet, `privilege` |
| `936665e` | nouvelle crate `debugger` sans E/S : `Points` et leurs bornes, `Region`, vues (`inspect`, `DebugView`, `StopView::describe`) ; 8ᵉ `clippy.toml`, `host_io_ban` à 8 crates, étape WebAssembly de la CI |
| `3eb2038` | `cpu-rv32` : lien de documentation sans ambiguïté |
| `9548cbf` | `host` : les points sur le fil de l'émulateur (`Command::Breakpoint`, `Command::Watchpoint`, `CommandError::Point`), l'arrêt qui met en pause et se dit, `Handle::inspect` de la vie qui tourne ou de la machine finie (`Engine::into_machine`) ; rejeu arrêté sept fois, « identique » |
| `f266740` | enveloppe Tauri : `power_on` et `open_replay` prennent les points, commande `inspect` hors du fil de l'interface |
| `7993754` | `app` : le panneau, l'arrêt dans le statut et l'annonce, les points gardés par le réducteur ; FR et EN |
| `1ec58ae`, `ca5e56a` | corrections de la première relecture |
| `503f54c` | ADR 0016, README (anglais et français), règle du point d'arrêt dans la spec |
| `ac2ec67` | cette note |
| `5985fa5` … `e492bb3` | corrections de la relecture de la PR #21 (section suivante) |

## Première relecture

Une relecture du code (`code-reviewer`) a trouvé deux points importants et quatre mineurs, tous corrigés, chacun avec
un test qui échoue sans le correctif.

- **I1** (`1ec58ae`) : un point d'arrêt arrêtait la machine quand une interruption allait être prise d'abord, ou
  pendant un `wfi` qu'une ligne masquée finissait. Corrigé : l'arrêt n'a lieu que si le processeur va exécuter
  l'instruction (`Hart::executes_next`). Avec une interruption, la machine s'arrête au retour du gestionnaire.
- **I2** (`1ec58ae`) : une exécution lancée après un pas s'arrêtait de nouveau sur le point que ce pas venait
  d'atteindre. Désormais `step` pose `breakpoint_left`, comme GDB enjambe le point dont il repart.
- **M3** (`1ec58ae`) : un point de surveillance retiré laissait un accès noté. `set_watches` oublie un accès qu'aucun
  point ne couvre plus.
- **M4** (`1ec58ae`) : la ROM n'était pas surveillée par son alias du bus de données. `canonical` ramène aussi
  l'alias DROM.
- **M5** (`ca5e56a`) : le panneau gardait la vue d'un lancement précédent, et une vue arrivée en retard pouvait
  remplacer la dernière demandée. Correctifs : `key={state.current}`, et un drapeau `wanted` dans le nettoyage de
  l'effet.
- **M6** (`ca5e56a`) : deux défauts du panneau.
  - Il interrogeait un lancement qui n'avait allumé aucune vie (`life` 0).
  - Pendant que l'appareil tourne, il ne relisait pas quand l'utilisateur demandait un autre code ou une autre
    mémoire (`askAgain`, `showCodeAt`, `showMemoryAt`).

## Relecture de la PR #21

Aucun point bloquant, deux importants et cinq mineurs, tous corrigés. Chaque correctif de comportement a un test qui
échoue sans lui ; I2 est une extension de la preuve, sans correctif.

| Commit | Constat |
|---|---|
| `5985fa5` | I1 |
| `557019a` | M2 |
| `b758fa1` | M1 |
| `5c664a3` | M3 |
| `fcb92e0` | M4 |
| `5807ee3` | M5 |
| `e492bb3` | I2 |

- **I1** : le désassemblage décodait toute page de flash projetée, une page de données comprise lue par l'alias
  d'instructions, soit des octets de flash presque réversibles. Le bus note désormais, sur le chemin du fetch, les
  pages de 64 Kio de la flash d'où le processeur a chargé une instruction (`SocBus::executed`, un `PageSet` de
  256 bits).
  - La note est posée quand le cache de décodage manque, jamais relue pendant l'exécution, gardée à travers les
    resets. Une programmation ou un effacement retire de la note les pages qu'il change (`settle_signals`).
  - `peek` ne donne aucun octet d'une autre page : `Peeked::NotExecuted`. La ligne du désassemblage porte
    `notExecuted`, et l'interface dit « page de la flash non exécutée : non lue ». Un point d'arrêt peut s'y poser.
  - L'ADR 0016 §6 et 4a-D6 sont en accord.
  - Tests :
    - `soc-esp32c3` : `a_peek_gives_a_flash_byte_only_from_a_page_the_cpu_fetched_from` et
      `a_page_the_flash_rewrote_is_not_executed_any_more` ;
    - `debugger` : `a_data_page_read_through_the_instruction_window_gives_no_line`, un programme qui lit une page de
      données et exécute une page de code ;
    - Vitest : ligne non exécutée.
- **I2** : `stopping_with_interrupts_wfi_and_a_time_limit_changes_nothing_of_the_run` compare une exécution coupée à
  chaque point à une exécution sans point (empreinte, cycles, instructions, temps, registres, mémoire). Le programme
  couvre :
  - une alarme périodique du SYSTIMER qui interrompt, sur la ligne 4 ;
  - un point d'arrêt à l'entrée du gestionnaire et sur chaque instruction de la boucle, l'instruction interrompue
    comprise (elle est lue dans `mepc`, et la machine s'y arrête au retour) ;
  - un `wfi` qu'une ligne masquée finit ;
  - deux points de surveillance ;
  - `run_stopping(FAR, Some(1 ms))` à chaque appel ;
  - les deux tarifs, `datasheet` et `qemu-icount`.

  Côté `host`, `a_replay_with_console_input_stopped_at_points_is_identical` rejoue une session enregistrée avec une
  entrée de console (`hi\r`, posée pendant un arrêt). Le rejeu est arrêté douze fois (le `addi` de `e`, les trois
  lectures et les huit rangements de la FIFO) et donne « identique ».
- **M1** : un point d'arrêt sur un alias du bus de données (alias DROM de la ROM, vue D de la SRAM, fenêtre de données
  de la flash) n'arrêtait jamais.
  - Il est refusé par `PointError::DataBusAlias`, qui donne l'adresse du bus d'instructions
    (`region::instruction_bus_twin`).
  - Son genre est `dataBusAlias` (`PointError::kind`, repris par `CommandError::kind` et l'enveloppe).
  - L'interface le dit en français et en anglais (`error.dataBusAlias`).
  - Tests dans `debugger`, `host` et l'enveloppe ; Vitest en FR et EN.
- **M2** : un pas jetait l'accès surveillé qu'une passe arrêtée pour une raison de la machine avait laissé en attente.
  `Esp32C3::step_stopping` le rend d'abord, comme arrêt, sans rien exécuter, et `Engine::step` l'appelle. Tests dans
  `soc-esp32c3` et `host`.
- **M3** : la vue mémoire se repliait sur `0x00000000` au bout de l'espace d'adresses. Sa longueur est bornée à la
  fin de l'espace, et `memoryRows` s'arrête aussi à `0xffffffff`. Tests dans `debugger` et Vitest.
- **M4** : « 1 octets » et « 1 bytes ». `bytesText` choisit `debug.bytes.one` ou `debug.bytes.other` dans l'arrêt
  d'un point de surveillance et dans sa ligne. Test Vitest `debug-views.test.ts`, en FR et EN.
- **M5** : l'interface recopiait la borne de 4 096 octets. `parseLength` ne vérifie plus que la forme : un nombre
  décimal qui tient sur 32 bits. Une longueur hors bornes va à l'hôte, qui la refuse avec ses bornes (4a-R8). Tests
  Vitest.

Les messages des douze premiers commits ont été réécrits par la coordination, pour retirer des lignes de trailer.

## Vérifié en local

Conditions : `CARGO_BUILD_JOBS=3`, aucun conteneur Docker.

- **Formatage et lint** :
  - `cargo fmt --all` ;
  - `tools/ci/clippy.sh … --all-targets --all-features -D warnings` sur `soc-esp32c3`, `debugger`, `host`, et
    `cpu-rv32` et `emu-cli` avant la seconde relecture.
- **Documentation** : `cargo doc --no-deps` avec `-D warnings` de `soc-esp32c3`, `debugger` et `host`.
- **Tests Rust** après la relecture de la PR #21 :
  - `cpu-rv32` : 154 ;
  - bibliothèque `soc-esp32c3` : 747 ;
  - `debugger` : 21 ;
  - bibliothèque `host` : 335, plus ses tests d'intégration non ignorés ;
  - tests non ignorés de `emu-cli`, `session`, `console-script`, `boards` et `lockstep`.
- **WebAssembly** : `cargo check --target wasm32-unknown-unknown` de `debugger`, `soc-esp32c3`, `cpu-rv32`.
- **Enveloppe Tauri** : `cargo fmt`, `clippy -D warnings`, 19 tests.
- **Application** : `pnpm lint`, `pnpm typecheck`, Vitest (19 fichiers, 120 tests), `pnpm build`.
- **Avec la vraie ROM**, récupérée dans un dossier temporaire, en release :
  - `soc-esp32c3` `real_rom` : 19/19 ;
  - `host` `real_rom` : 2/2.

  Leurs empreintes figées n'ont pas bougé : la note des pages exécutées ne change rien de l'exécution.

Laissés à la CI :

- les 30 tests ignorés de `run_cli` du job `rom-boot` ;
- les suites de `reference-boot`, qui demandent les firmwares de référence construits dans Docker ;
- la suite complète de l'espace de travail ;
- la couverture.

La preuve que l'arrêt ne change rien ne dépend pas de ces suites :

- **dans `soc-esp32c3`** :
  - `stopping_at_points_changes_nothing_of_the_run` ;
  - `stopping_with_interrupts_wfi_and_a_time_limit_changes_nothing_of_the_run` ;
- **dans `host`** :
  - `a_replay_stopped_at_points_is_identical` ;
  - `a_replay_with_console_input_stopped_at_points_is_identical`.

## Décisions prises sans l'utilisateur

1. **La flash n'est jamais montrée** (ADR 0016, point 6) :
   - la vue mémoire dit « contenu de la flash, non montré » pour les deux fenêtres ;
   - un point de surveillance y est refusé ;
   - le désassemblage lit la seule fenêtre d'instructions, et dans elle les seules pages de 64 Kio d'où le processeur
     a chargé une instruction pendant cette vie (décision de la coordination, I1 de la PR #21). Il n'en donne que le
     texte décodé, sans encodage ni bits (une instruction illégale s'écrit `illegal`) ;
   - rien de ce que lit le débogueur n'entre au journal, dans un fichier ou dans une session.
2. **La note des pages exécutées** :
   - elle se tient par page physique de flash, pas par entrée de MMU ;
   - elle est gardée à travers les resets, car elle est l'histoire de la machine ;
   - elle oublie une page qu'une programmation ou un effacement change, car ce qu'elle tient n'est plus ce qui a été
     exécuté ;
   - son coût est d'un test et d'un bit posé par octet lu au fetch, sur un chemin que le cache de décodage n'emprunte
     qu'à un défaut. Aucune mesure n'a été faite : le dépôt n'a pas de banc.
3. **Lecture seule** : ni registre, ni CSR, ni mémoire ne s'écrivent depuis le débogueur. Une écriture changerait
   l'exécution sans entrer au journal. SP4d tranchera pour les paquets `G`, `P` et `M` de GDB.
4. **Un registre de périphérique n'est jamais lu** : sa lecture a des effets (FIFO, bits effacés). La vue dit
   « registres de périphériques, non lus (une lecture peut les changer) ».
5. **Bornes** : 64 points d'arrêt, 16 points de surveillance de 1 à 4 096 octets, 1 024 octets de mémoire et
   64 lignes de code par vue. Le panneau demande 256 octets et 24 lignes. Les bornes sont celles de l'hôte seul ;
   l'interface ne vérifie que la forme d'un nombre (M5).
6. **Les points appartiennent à l'émulateur et à l'interface**, qui les garde d'un lancement à l'autre (4a-D8).
   Ils ne vont dans aucune session.
7. **La machine d'une vie finie est gardée** jusqu'à la suivante, pour être lue. Elle ne tient aucun fichier.
8. **Lignes du journal** : un arrêt écrit « inkemu: breakpoint at … » ou « inkemu: watchpoint: … ». La valeur d'un
   accès surveillé n'y figure que s'il porte sur une région permise.
9. **Le panneau se relit seul quand l'appareil ne bouge pas** : pause, arrêt, pas, fin. Pendant qu'il tourne, il ne
   relit qu'à la demande (« Actualiser », autre code, autre mémoire, point posé) et dit que la vue peut être ancienne.
10. **Un pas enjambe le point d'arrêt où il s'arrête** : l'appel suivant repart au-delà, sémantique de GDB. Un pas
    rend d'abord un accès surveillé en attente, sans exécuter (M2).
11. **Un point d'arrêt sur un alias du bus de données est refusé** avec l'adresse du bus d'instructions, plutôt que
    converti : l'utilisateur voit l'adresse que le PC prendra (M1). Le genre `dataBusAlias` donne à l'interface une
    phrase à elle ; l'adresse reste dans le détail de l'hôte.
12. **Le code avant le PC est une meilleure supposition** : avec des instructions de 16 bits, remonter est ambigu.
    Le désassemblage part de l'adresse la plus lointaine (au plus 8 lignes, un tiers de la vue) d'où le décodage
    tombe exactement sur le PC, et du PC lui-même quand aucune ne le fait.

## Laissé aux héritiers

| Sujet | Héritier |
|---|---|
| Moniteur de ressources ; 6 lignes de registres d'alimentation acceptées sans effet ; consommation pendant le sommeil profond | SP4b |
| Câble, batterie et carte pendant une vie ; RAM brute du SSD1677 ; compteur de partiels ; animation ; persistance de la dalle ; borne de la mémoire des changements de carte non écrits (ADR 0014) ; installer en gardant la NVS ; profils de l'utilisateur | SP4c |
| Symboles ELF, pile d'appels, tâches FreeRTOS ; serveur GDB sur les arrêts de SP4a ; écriture de registres et de mémoire (paquets `G`, `P`, `M`) ; montrer les constantes des sections d'un ELF fourni, et le code des pages pas encore exécutées que l'ELF dit être du code ; `inspect` depuis l'interface | SP4d |
| Scénarios YAML et régression visuelle, remplacement de `inkemu-console-script 1`, scénario → session, vidage de la console USB-Serial-JTAG, SIGTERM | SP4e |
| Déclencheurs de la puce (`tselect`, `tdata1`…), `rv32mi-p-breakpoint`, `ASSIST_DEBUG`, `SENSITIVE`/PMS | SP4f |
| Lire un registre de périphérique sans effet (un second chemin de lecture par périphérique) | à la demande |
| Formulaires du panneau cachés tant qu'aucune vue n'est lue (appareil qui tourne, avant la première actualisation) | avec la refonte du panneau de SP4d |
| Mesurer le coût de la note des pages exécutées sur un firmware réel | avec le premier banc de performance |

## Limites de preuve

- **L'application n'a pas été pilotée à la souris** dans une fenêtre Tauri en marche. Elle est tenue par :
  - Vitest (panneau, formats, réducteur, bannière) ;
  - les tests de l'enveloppe ;
  - une construction de production.

  Le chemin qu'elle prend dans `host` (`inspect`, points, arrêt) est tenu par les tests de `host`.
- **Les suites de la CI ne sont pas toutes passées en local** : `rom-boot`, `reference-boot`, la suite complète et la
  couverture tournent sur GitHub. La preuve du non-changement (empreinte, rejeu) est dans des tests locaux passés.
- **Le coût de la note des pages exécutées n'est pas mesuré** : il est argumenté (décision 2).
- **Le code avant le PC peut être faux** : il est une supposition (décision 12), le code à partir du PC ne l'est pas.
- **Le code de la flash n'est visible qu'une fois exécuté** : une fonction pas encore appelée n'est pas désassemblée.
  Un point d'arrêt peut s'y poser, mais l'utilisateur ne voit pas le code avant qu'il tourne (I1, SP4d avec l'ELF).
- **Les points n'ont été posés que sur nos programmes** : aucun firmware réel n'a été arrêté (règle 4a-R4, tests
  sur nos programmes seulement).

## Cartographie

Fichiers de la cartographie du projet à mettre à jour (la mémoire n'a pas été modifiée par ce plan) :

| Fichier | Ce qui change |
|---|---|
| `architecture_overview.md` | SP4a fait : crate `debugger` (sans E/S, wasm), `run_stopping` qui partage `run_loop` avec `run_within` (crochets `Stops`, `NoStops` d'arrêt `Infallible`), points sur le fil de l'émulateur, machine d'une vie finie gardée, flash lisible par le débogueur limitée aux pages exécutées (note du bus sur le chemin du fetch) ; ADR 0016 ; huit `clippy.toml` |
| `backend_models.md` | `cpu-rv32` : `disasm` (`ABI_NAMES`, `register_name`, `csr_name`, `disassemble`), `Hart::executes_next` ; `soc-esp32c3` : `Watch`, `WatchOn`, `Access`, `WatchHit`, `DebugStop`, `RunStop`, `Peeked` (dont `NotExecuted`), `Stops`/`NoStops`/`DebugStops`, `PageSet` (`pub(crate)`, partagé par `Extmem` et le bus) ; `debugger` : `Points`, `PointError` (dont `DataBusAlias`, et `kind()`), `PointsRequest`, `WatchRequest`, `Region`, `ViewRequest`, `DebugView`, `CpuView`, `Line` (dont `not_executed`), `MemoryView`, `RegionSpan`, `StopView` ; `host` : `Command::Breakpoint`/`Watchpoint`, `CommandError::Point` (genres `point` et `dataBusAlias`), `Request` (énumération), `Launch.points`, `Status.stop`, `Output.stop` |
| `backend_services.md` | `soc-esp32c3` : `machine/debug.rs` (`set_breakpoints`, `set_watchpoints`, `run_stopping`, `step_stopping`, `take_watch_hit`, `peek`, `privilege`), `bus/debug.rs` (`note_access`, `set_watches`, `take_watch_hit`, `peek_byte` qui ne lit que les pages exécutées, `canonical`), `SocBus::executed` posé par `flash_read` au fetch et vidé par `settle_signals` sur `FlashWritten`, `step` qui pose `breakpoint_left` ; `debugger` : `points.rs`, `region.rs` (`instruction_bus_twin`), `view.rs` (`inspect`, vue mémoire bornée à la fin de l'espace) ; `host` : `Engine::set_points`/`into_machine`, `Engine::step` par `step_stopping`, `Worker.points/stop/ended`, `said_stop`, `Handle::inspect`/`Emulator::inspect`, `test_support` (`counting_device`) |
| `backend_controllers.md`, `api_routes.md` | enveloppe Tauri : commande `inspect`, `power_on` et `open_replay` avec `points`, `CommandDto::Breakpoint`/`Watchpoint`, genre d'erreur `dataBusAlias` ; lignes de journal « inkemu: breakpoint at … » et « inkemu: watchpoint: … » ; aucune option nouvelle de la CLI |
| `database_schema.md` | inchangé (les points ne vont dans aucune session ni aucun fichier) |
| `frontend_spa.md` | bouton « Débogueur » (`aria-expanded`, `aria-controls`) ; `DebugPanel` (registres, CSR, code, mémoire, points d'arrêt, points de surveillance ; relu quand l'appareil ne bouge pas, à la demande sinon) ; `debug-format.ts` (`parseLength` sur la seule forme, `memoryRows` qui s'arrête à `0xffffffff`), `debug-views.tsx` (`bytesText`, ligne « non exécutée » où un point d'arrêt peut se poser) et `debug-views.test.ts` ; arrêt dans `StatusPanel` et l'annonce ; réducteur : `points` ; i18n `debug.*` (dont `debug.code.notExecuted`, `debug.bytes.one`/`other`), `status.stop*`, `toolbar.debugger`, `error.point`, `error.dataBusAlias` ; `host.inspect` |
| `frontend_nextjs.md` | `debugger` compile pour wasm32 (étape de la CI) |
| `integrations.md` | étape WebAssembly de la CI avec `-p debugger` ; `host_io_ban` à 8 crates |
| `tech_debt.md` | les héritiers du tableau ci-dessus ; supposition du code avant le PC ; registres de périphérique non lus ; code de la flash visible seulement une fois exécuté ; coût de la note des pages exécutées non mesuré |
