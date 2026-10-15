# SP4b — Note de passation

Branche de SP4b, partie de `main` à `5a28566` (SP4a mergé). Branche locale : `sp4b-resource-monitor` (rien n'est
poussé). Découpage de SP4 : `docs/superpowers/notes/2026-10-15-sp4-decoupage.md` ; recherche :
`docs/research/sp4b-resource-monitor.md` ; spec : `docs/superpowers/specs/2026-10-15-sp4b-resource-monitor-design.md`
(décisions 4b-D1 à 4b-D13) ; plan : `docs/superpowers/plans/2026-10-15-sp4b-resource-monitor.md` (règles 4b-R1 à
4b-R8, tâches 1 à 9) ; ADR 0017.

## Résultat

Le moniteur de ressources est dans l'application. Un bouton « Moniteur » de la barre ouvre un panneau, relu chaque
seconde pendant que l'appareil tourne et une fois à chaque arrêt, qui montre ce que la machine a mesuré depuis la
mise sous tension :

- **le processeur** : fréquence, temps de l'appareil, cycles, instructions, vitesse émulée/réelle, resets ;
- **le temps** : exécution, attente en `wfi`, sommeil profond ; la charge (part du temps éveillé hors `wfi`) depuis la
  mise sous tension et sur la dernière fenêtre (la seconde écoulée entre deux relevés) ; le temps à chaque fréquence ;
- **la mémoire vive** : pour la SRAM, la SRAM0 et la mémoire RTC rapide, une carte des pages de 4 Kio « jamais
  écrite », « avant le dernier reset », « depuis », avec son texte, la plage écrite et le nombre d'écritures ;
- **l'activité** : interruptions levées par source et prises par ligne, accès du processeur aux registres par bloc de
  2 Kio nommé par le SVD, rafraîchissements de l'écran de la vie ;
- **la consommation estimée** de la puce, dite de la fiche technique (v2.4, tableaux 5-8 et 5-9) : une fourchette de
  courant par état et la charge consommée ; le temps aux fréquences sans valeur (20 et 40 MHz) est dit à part.

Observer ne change rien : même empreinte de trace, même relevé final quelle que soit la coupure, rejeu « identique ».
Aucune empreinte figée n'a bougé.

| Commit | Contenu |
|---|---|
| `0e18c34` | recherche, spec et plan de SP4b |
| `d645c22` | `emu-core` : `Ram::version`, `Ram::page_versions` |
| `a3d0ebd` | `soc-esp32c3` : `usage` (types publics), `machine/usage.rs` (`Counters`, `sleep_to`, `Esp32C3::usage`), `bus/usage.rs` (accès par bloc, pages des RAM), crochets dans `machine.rs` et `bus.rs` ; le programme interrompu des tests de SP4a passe dans `machine/test_programs.rs` |
| `f376a22` | nouvelle crate `monitor` (vue, noms, fiche technique) ; neuvième copie du `clippy.toml` (commentaire rendu générique dans les neuf), `host_io_ban` à 9 crates, étape WebAssembly de la CI |
| `a7a3303` | `host` : `resources.rs` (`Resources`), `Request::Monitor`, `Handle::monitor`, `Emulator::monitor`, rafraîchissements comptés |
| `1847876` | banc `crates/soc-esp32c3/tests/throughput.rs` (ignoré) |
| `e2727e5` | les six lignes d'alimentation passent à SP8, table régénérée |
| `f38c0e4` | ADR 0017 |
| `aab9dda` | enveloppe Tauri : commande `monitor` |
| `a522ff4` | `app` : `MonitorPanel`, `monitor-format.ts`, bouton de la barre, FR et EN |
| `5ff1d7d` | `app` : la légende de la carte des pages a son nom |
| ce commit et le suivant | README (anglais et français), cette note |

## Coût mesuré

Banc `crates/soc-esp32c3/tests/throughput.rs`, en release, lancé à la main
(`cargo test --release -p soc-esp32c3 --test throughput -- --ignored --nocapture`). Trois programmes de 200 millions
de cycles, le meilleur de cinq passes chacun, en millions d'instructions par seconde :

- `arithmetic` : additions et soustractions sur des registres ;
- `sram` : rangements et chargements en SRAM, la page versionnée à chaque rangement ;
- `registers` : lectures d'un registre de `SYSTIMER`, chaque instruction est un accès compté par bloc.

Le même fichier a été construit sur `main` (`5a28566`, arbre exporté par `git archive`) et sur la branche. Six tours
alternés, le 2026-10-15, sur une machine partagée avec d'autres travaux, avec une charge de 3 à 27 : le bruit d'un
tour à l'autre atteint 40 %.

| Programme | `main`, meilleur | branche, meilleur | `main`, médiane | branche, médiane |
|---|---|---|---|---|
| `arithmetic` | 25,3 | 25,3 | 22,4 | 23,6 |
| `sram` | 19,5 | 20,2 | 18,0 | 18,8 |
| `registers` | 17,8 | 17,0 | 16,0 | 15,1 |

Lecture :

- **`arithmetic` et `sram`** : aucune différence mesurable. C'est attendu : les comptes ne touchent pas le chemin
  d'une instruction, et les pages écrites sont les versions que la RAM tenait déjà.
- **`registers`** : au plus 5 % de moins, dans le bruit. Ce programme est le pire cas, puisque chacune de ses
  instructions est un accès de registre, qui coûte un incrément de plus. Dans un firmware réel, ces accès sont une
  petite part des instructions.

Le coût du compte est donc négligeable, que le panneau soit ouvert ou non. Panneau fermé, rien d'autre n'est fait ;
ouvert, un relevé par seconde. Fermé, le panneau n'est pas monté et ne demande rien (test `app.test.tsx`).

## Vérifié en local

Conditions : `CARGO_BUILD_JOBS=3`, aucun conteneur Docker.

- `cargo fmt --all --check` ; `tools/ci/clippy.sh … --all-targets --all-features -D warnings` sur `emu-core`,
  `soc-esp32c3`, `debugger`, `boards`, `monitor`, `host` ; `cargo doc --no-deps` avec `-D warnings` de `emu-core`,
  `soc-esp32c3`, `monitor`, `host`.
- Tests :
  - `emu-core` ;
  - bibliothèque `soc-esp32c3` : 758, dont 8 de `machine::usage` ;
  - `monitor` : 18 ;
  - bibliothèque `host` : 341, dont 3 de `resources` et 2 du moniteur dans `device` ;
  - `boards` (`host_io_ban`), `debugger`, `session`, `lockstep`, `console-script`, `emu-cli` (tests non ignorés).
- WebAssembly : `cargo check --target wasm32-unknown-unknown` de toute la liste de la CI, `monitor` compris.
- Enveloppe Tauri : `cargo fmt`, `clippy -D warnings`, 21 tests.
- Application : `pnpm lint`, `pnpm typecheck`, Vitest (21 fichiers, 137 tests), `pnpm build`.
- Avec la vraie ROM (les trois révisions récupérées par `inkemu rom fetch --all` dans un dossier temporaire), en
  release : `soc-esp32c3` `real_rom` 19/19, `host` `real_rom` 2/2. Les empreintes figées n'ont pas bougé.

Laissés à la CI : les tests ignorés de `run_cli` (`rom-boot`), les suites de `reference-boot`, la suite complète de
l'espace de travail, la couverture.

La preuve qu'observer ne change rien ne dépend pas de ces suites. Elle tient en deux tests passés en local :
`reading_the_usage_after_every_cut_changes_nothing_of_the_run_nor_of_the_counts` (`soc-esp32c3`, deux programmes,
deux tarifs) et `a_replay_read_by_the_monitor_all_along_is_identical` (`host`).

## Décisions prises sans l'utilisateur

1. **Le compte ne s'arrête jamais** (4b-D3, ADR 0017) : un relevé dit ce qui s'est passé depuis la mise sous tension,
   démarrage compris. Un compte ouvert seulement panneau ouvert aurait coûté zéro fermé, mais n'aurait rien dit du
   démarrage. Le coût est mesuré (section précédente).
2. **Rien par instruction** (4b-D2, 4b-R4) : le temps d'exécution est la différence entre le temps éveillé et
   l'attente. Le temps par fréquence se ferme au changement d'horloge. L'attente et le sommeil profond sont comptés
   aux quatre `sleep_until`, que `Esp32C3::sleep_to` remplace.
3. **La mémoire au grain de la page de 4 Kio**, par les versions que la RAM tient pour le cache de décodage
   (4b-D5) :
   - « depuis le dernier reset » et « avant » sont distingués par une marque posée à chaque reset ;
   - « occupation » au sens du tas attend les symboles (SP4d) ;
   - le contenu n'est jamais montré.
4. **L'activité des périphériques** est comptée en accès de registre par bloc de 2 Kio, la plus petite granularité
   qui sépare les blocs du SVD (`APB_CTRL` et `RNG` restent ensemble), plus les interruptions et les
   rafraîchissements de l'écran (4b-D6). Le détail par périphérique (octets SPI, transactions I2C, conversions ADC,
   blocs de la carte) demanderait des crochets dans les modèles, dans la zone de SP3 : il n'est pas fait.
5. **Une source levée** est un front montant vu par le contrôleur quand il échantillonne les niveaux. Une impulsion
   de moins d'une instruction n'est pas comptée, comme le contrôleur ne la voit pas (ADR 0003).
6. **La fenêtre récente** s'étend du relevé précédent servi au panneau, dans la même vie, à celui-ci. L'hôte garde le
   relevé, `monitor` calcule (4b-D7). Une nouvelle vie efface la fenêtre et le compte de l'écran.
7. **La consommation est la fiche technique v2.4, puce seule, en fourchette** (4b-D8) :
   - de « toutes horloges de périphériques coupées » à « toutes en marche » ;
   - sommeil profond à 5 µA ;
   - aucune valeur aux autres fréquences, dont le temps est dit à part ;
   - le PDF lu est identifié par son SHA-256 dans la recherche.
8. **Les six registres d'alimentation passent à SP8** (4b-D9) : la fiche ne chiffre aucun domaine. Leur description
   le dit, la table est régénérée. `RTC_CNTL.SLP_REJECT_CONF` et `DIG_ISO` gardent l'héritier « SP4 » : le refus du
   sommeil léger et l'isolement des broches ne sont pas des sujets du moniteur (héritier proposé : SP4c).
9. **Batterie et câble ne sont pas dans SP4b**, contrairement au découpage (4b-D10) :
   - ils sont fixés à la mise sous tension (ADR 0013), donc une valeur figée n'est pas une mesure ;
   - leurs lecteurs (`boards`, catalogue de `host`) sont la zone de SP3 en parallèle ;
   - SP4c les rend vivants et les ajoutera au panneau.
10. **Le commentaire des `clippy.toml`** ne nomme plus les crates : il renvoie à `CRATES` de `host_io_ban.rs`. Ajouter
    une crate ne demande plus de toucher les neuf copies.
11. **Le banc** est un test ignoré : il mesure l'hôte, il ne doit pas échouer en CI. Il dit ce qu'il mesure et
    n'affirme aucune vitesse.

## Laissé aux héritiers

| Sujet | Héritier |
|---|---|
| Batterie et câble au moniteur, vivants ; blocs lus et écrits de la carte ; durée et nature de chaque rafraîchissement, partiels depuis le dernier complet | SP4c |
| Héritier des lignes `RTC_CNTL.SLP_REJECT_CONF` et `DIG_ISO`, encore « SP4 » (proposé : SP4c) | SP4c |
| Tâches FreeRTOS, pile de chaque tâche et son plus haut niveau, tas | SP4d |
| Rapport du moniteur à la fin d'un scénario sans écran, depuis la CLI | SP4e |
| Consommation par domaine d'alimentation, six registres d'alimentation | SP8 |
| Détail par périphérique (octets SPI, transactions I2C, conversions ADC) | à la demande, hors de la zone de SP3 |
| Une lecture servie par le fil (débogueur, moniteur) repousse la passe suivante d'au plus `PACE_WAIT` (5 ms) en temps réel ; un client qui lirait sans pause retiendrait l'exécution. Le panneau lit une fois par seconde. | à la demande, avec le prochain changement de la boucle du fil |

## Limites de preuve

- **L'application n'a pas été pilotée à la souris** dans une fenêtre Tauri. Elle est tenue par :
  - Vitest : panneau, formats, bouton, ouverture et fermeture dans `App` ;
  - le test de l'enveloppe ;
  - une construction de production.
- **Le banc a tourné sur une machine chargée.** Le bruit dépasse l'écart mesuré : la conclusion est « négligeable »,
  pas un chiffre précis.
- **Les comptes n'ont été vérifiés que sur nos programmes** : attente, sommeil profond, horloges, interruptions,
  blocs, pages. Aucun firmware réel n'a été observé au moniteur. CrossPoint tourne en local dans `real_firmware.rs`,
  qui n'a pas été relancé.
- **Les valeurs de la fiche technique** sont lues dans le PDF v2.4 ; aucune mesure sur matériel ne les confirme.

## Cartographie

Fichiers de la cartographie du projet à mettre à jour (la mémoire n'a pas été modifiée par ce plan) :

| Fichier | Ce qui change |
|---|---|
| `architecture_overview.md` | SP4b fait : crate `monitor` (sans E/S, wasm) ; comptes de la machine tenus là où passent le temps et les événements (`sleep_to`, `retune`, `turn`, `select_interrupt`, `reset`, `mmio_read`/`mmio_write`), relevé en lecture seule `Esp32C3::usage` ; pages écrites par les versions de pages des RAM ; l'hôte garde le relevé précédent (`host::resources`) ; ADR 0017 ; neuf `clippy.toml`, commentaire générique |
| `backend_models.md` | `emu-core` : `Ram::version`, `Ram::page_versions` ; `soc-esp32c3::usage` : `Usage`, `ClockUse`, `BlockUse`, `MemoryUse`, `BLOCK_BYTES`, `SOURCES`, `LINES` ; `monitor` : `MonitorView`, `TimeView`, `RecentView`, `ClockView`, `MemoryView`, `MemoryRegion`, `PageState`, `SourceView`, `LineView`, `BlockView`, `HostSide`, `PanelUse`, `Refresh`, `PowerView`, `PowerRow`, `PowerState`, `ClockTime` ; `host` : `Resources`, `Request::Monitor` |
| `backend_services.md` | `soc-esp32c3` : `machine/usage.rs` (`Counters`, `Asleep`, `sleep_to`, `usage`), `bus/usage.rs` (`RegisterAccesses`, `note_register`, `block_uses`, `memory_versions`, `memory_uses`), `machine/test_programs.rs` (tests) ; `monitor` : `view.rs` (`view`), `names.rs` (`block_peripherals`, `source_name`), `power.rs` (`estimate`, `SOURCE`, `DEEP_SLEEP_MICROAMPS`) ; `host` : `resources.rs`, `Handle::monitor`, `Emulator::monitor`, `Worker::monitor` |
| `backend_controllers.md`, `api_routes.md` | enveloppe Tauri : commande `monitor(start)` ; banc `cargo test --release -p soc-esp32c3 --test throughput -- --ignored --nocapture` ; aucune option nouvelle de la CLI |
| `database_schema.md` | inchangé (rien n'entre dans une session ni dans un fichier) |
| `frontend_spa.md` | bouton « Moniteur » (`aria-expanded`, `aria-controls`) ; `MonitorPanel` (processeur, temps et charge, fenêtre, temps par fréquence, carte des pages en `role="img"` avec texte et légende, interruptions, lignes, blocs, écran, consommation) relu chaque seconde pendant la course, une fois à l'arrêt, jamais fermé ; `monitor-format.ts` (`count`, `percent`, `duration`, `current`, `charge`, `kibibytes`, `speedRatio`, au format de la langue) ; i18n `monitor.*`, `toolbar.monitor` ; `host.monitor` ; types `MonitorView` et parties |
| `frontend_nextjs.md` | `monitor` compile pour wasm32 (étape de la CI) |
| `integrations.md` | fiche technique ESP32-C3 v2.4 (tableaux 5-8, 5-9, SHA-256 du PDF dans la recherche) ; étape WebAssembly avec `-p monitor` ; `host_io_ban` à 9 crates |
| `tech_debt.md` | les héritiers du tableau ci-dessus ; lignes `SLP_REJECT_CONF` et `DIG_ISO` sans plan nommé ; lecture servie qui repousse une passe ; banc mesuré sur machine chargée |
