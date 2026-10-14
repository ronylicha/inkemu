# SP1b-4c — Note de passation

Plan : `docs/superpowers/plans/2026-10-14-sp1b-4c-lockstep-boot.md`. Spec : `docs/superpowers/specs/2026-10-14-sp1b-4c-design.md` (H1–H11). ADR 0008. Tableau : `docs/lockstep-differences.md`.

## Résultat

- `inkemu-lockstep` compare `flash_hello.bin` à QEMU (`espressif/idf:v5.5.2`, build `esp_develop_9.2.2_20250817`) de la première instruction de la ROM au reset que demande `esp_restart` : **3 402 169 instructions comparées**, fin de fenêtre **« reset (QEMU and inkemu) »**, 1 112 interruptions prises par les deux côtés à la même instruction, 50 blocs réimprimés et 10 blocs préemptés écartés. `app_main` vers l'instruction 2,396 M, la première interruption (ligne 3, le yield de FreeRTOS) vers 2,369 M, le premier tick (ligne 4) vers 2,423 M. Mesuré deux fois en local, compte identique ; durée : 84 s sur l'hôte de développement, la vitesse de QEMU étant la limite (environ 40 000 instructions par seconde comparées).
- 24 lignes de différences admises : 4 masques (la ligne 1 recopie aussi les bits de `MPP` des lectures de `mstatus`), 19 recopies, 1 réglage `QemuCompat`. Aucune resynchronisation. Leurs nombres d'usages sont dans le tableau.
- Le job `reference-boot` épingle le compte et la fin (`--expect-compared 3402169 --expect-end reset`). Le compte a été mesuré en local : si la CI en donne un autre avec la même image, le contrôleur le réépingle depuis le journal du job (une ligne du workflow, une du README, que `crates/lockstep-cli/tests/readme.rs` tient égales).

## Décisions de ce plan (R1–R14)

Détaillées dans le plan ; les plus engageantes :

- **R1** l'unité est la frontière d'instruction : `Esp32C3::step` exécute une instruction et rend la machine à la frontière de la suivante (interruption prise, `wfi` traversé). `run_within` et `step` partagent un même tour de boucle ; un test compare un `run_until` et une suite de `step` jusqu'à l'empreinte de trace.
- **R3** deux sortes de blocs écartés : réimprimés (piège 1 de l'oracle) et **préemptés** — QEMU imprime un bloc en y entrant, puis prend l'interruption avant d'exécuter l'instruction ; le `-d int` qui suit a pour `epc` le PC du bloc. Découvert en local à l'instruction 2 511 330 ; sans cette règle, ces interruptions (10 sur 1 112 dans la fenêtre) divergeaient d'une instruction.
- **R4** `qemu-icount` : 32 cycles de la base de temps toutes les 25 instructions, le reste en vingt-cinquièmes ; interruptions et attentes gratuites ; CPI 1.
- **R5** `-icount shift=3,sleep=off` : avec `sleep=on`, deux exécutions de QEMU différaient de 21 instructions sur la première vie (horloge virtuelle qui suit l'hôte pendant les `wfi`).
- **R6** fins de fenêtre et blocs absents ; une ligne (ii) peut admettre des accès à un bloc absent (APB_SARADC par la source d'entropie du bootloader, ligne 18 ; BB par `rtc_sleep_pu`, ligne 13). Sans elle, la fenêtre s'arrêtait à l'instruction 579 249, dans le bootloader : la spec supposait que `hello_world` ne touche aucun bloc absent avant `app_main`, ce qui est faux.
- **R8** la seule ligne (iv), le tick précoce du SYSTIMER.
- **R10** la fenêtre de la CI est toute la première vie, pas seulement `app_main` et le premier tick : elle finit sur une raison de H4 (le reset) plutôt que sur une limite arbitraire.

## Ce que le lockstep a tranché, et ce qu'il a seulement admis

Lignes du tableau de la note SP1b-2 (`2026-10-11-sp1b-2-carryover.md`) :

- **Rien n'y diverge hors des registres** : le cœur, le décodage, les CSR, les exceptions, la prise d'interruption (ligne, `mepc`, `mcause`, instant) et la lecture de la flash à travers la MMU et le cache sont identiques à QEMU sur toute la fenêtre.
- **Admises, pas tranchées** (QEMU n'est pas une référence pour elles) : ligne 3 (`ICACHE_SYNC_CTRL`, notre ligne 22), 6 (CPU sur la PLL au reset, ligne 20), 7 et 8 (regi2c, ligne 12), 9 (SFDP, ligne 7), 17 (eFuse, ligne 10), 20 (aléa, ligne 17), 21 (`SPI0`, ligne 8), 22 (`EP1_CONF`, ligne 19). Les recopies empêchent ces registres de faire diverger le flot ; elles ne disent pas qui a raison.
- **Sans traitement** : lignes 1, 2, 4, 5, 10 à 16, 18 et 19 ; aucune recopie ni aucun masque n'a été nécessaire pour elles dans la fenêtre : leurs registres n'y sont pas lus, ou les deux côtés y lisent la même valeur. L'opcode d'effacement de toute la puce (`FLASH_CE`, `C7` contre `60` dans QEMU) n'est pas exercé : `hello_world` n'efface pas la flash.
- **Dette soldée** : la latence d'interruption est réglable (`SocConfig::interrupt_latency_cycles`, 0 sous `qemu-icount`).

## Laissé aux héritiers

- **SP1b-4d** :
  - traverser le reset : QEMU réinitialise toute la machine, inkemu seulement le CPU (ligne `Saved PC`, `rtc_clk_init` différent au second démarrage) ; repartir d'un état commun après le reset ;
  - CrossPoint, en local et daté (il ne peut être ni distribué ni téléchargé) ; sa fenêtre s'arrêtera au premier accès à un bloc absent (APB_SARADC pour la température, puis I2C0), que les lignes 13 et 18 ne couvrent pas : elles nomment les seuls registres de la source d'entropie et de `rtc_sleep_pu` ;
  - la build QEMU `20260417` (interruptions perdues avec `MIE` = 0 corrigées, blocs réimprimés plus nombreux) : rien dans la fenêtre de `hello_world` n'en a eu besoin.
- **Sous-projet 2 / 4** : `Esp32C3::step`, `x`, `set_x`, `csr` et `Hart::take_boundary` sont l'API du débogueur et du serveur GDB.
- **Sous-projet 8** : chaque ligne (ii) est une question pour un appareil (valeur de reset, ce que lit la puce).

## Limites de preuve

- Vérifié en local : `cargo fmt`, clippy et rustdoc sans avertissement sur les crates touchées, les tests de `cpu-rv32`, `soc-esp32c3` (bibliothèque), `lockstep`, `inkemu-lockstep` et `boards::host_io_ban`, la compilation WebAssembly de `lockstep`, `soc-esp32c3` et `cpu-rv32`, et deux exécutions complètes du lockstep contre QEMU dans Docker.
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace`, les suites ignorées (vraie ROM, firmwares de référence, dont les empreintes de trace épinglées que le refactor de la boucle ne doit pas changer), la couverture, l'étape de la CI elle-même (tirage de l'image, place disque avec le libérateur rendu inconditionnel, durée).
- Le pas `Esp32C3::step` ne prend pas de limite de cycles ni de temps : il s'arrête aux bornes d'un sommeil (`SLEEP_SLICE`, `MAX_FRUITLESS_WAKES`), comme `run_within`.
- La règle du branchement sur lui-même (R3) ne distingue pas une boucle `j .` d'un bloc réimprimé à la même instruction ; aucune n'a été rencontrée.
