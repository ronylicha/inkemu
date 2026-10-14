# SP1b-4c — Note de passation

Plan : `docs/superpowers/plans/2026-10-14-sp1b-4c-lockstep-boot.md`. Spec : `docs/superpowers/specs/2026-10-14-sp1b-4c-design.md` (H1–H11, et sa correction du 2026-10-14). ADR 0008. Tableau : `docs/lockstep-differences.md`.

## Résultat

- `inkemu-lockstep` compare `flash_hello.bin` à QEMU (`espressif/idf:v5.5.2`, build `esp_develop_9.2.2_20250817`) de la première instruction de la ROM au reset que demande `esp_restart` : **3 402 169 instructions comparées**, fin de fenêtre **« reset (QEMU and inkemu) »**, 1 112 interruptions prises par les deux côtés à la même instruction, 50 blocs réimprimés (jamais deux de suite) et 10 blocs préemptés écartés, une ligne de trace ignorée (le message de QEMU sur la flash). `app_main` vers l'instruction 2,396 M, la première interruption vers 2,369 M, le premier tick vers 2,423 M. Le compte est le même qu'avant les correctifs ; deux exécutions vivantes et la trace capturée donnent le même compte et le même usage des lignes. Durée : 85 s environ, la vitesse de QEMU étant la limite (environ 40 000 instructions par seconde comparées).
- **24 lignes, toutes utilisées dans ce qu'elles déclarent** : 4 paires (i), 18 lignes (ii) qui nomment chaque registre avec ses bits mesurés (dont deux compteurs admis dans une borne), 3 réglages `QemuCompat` (lignes 5, 15 et 24). Aucune resynchronisation.
- Le job `reference-boot` épingle le compte et la fin (`--expect-compared 3402169 --expect-end reset`), imprime l'usage de chaque ligne, échoue (code 5) si une ligne ne sert pas ou sert hors de sa déclaration, puis refait la comparaison avec une faute mise exprès dans inkemu (`--perturb-after 1000000`, un bit de `t0`) et exige la divergence à l'instruction 1 000 000 avec l'instruction nommée. Mesuré en local : code 1, « divergence after 1000000 compared instructions », `x5/t0 … <<`.

## Correctifs de la revue (2026-10-14)

Deux revues ont trouvé que le lockstep pouvait passer avec un modèle faux. Ce qui a changé :

- **Copies exactes (R15).** Une ligne (ii) nomme chaque registre et les bits mesurés différents, rien de plus ; à un chargement d'un mot, les bits hors masque doivent être égaux, ceux du masque sont pris à QEMU, la valeur d'inkemu reste dans l'historique du rapport. Plus aucune plage de registres. Les masques sont l'union des bits mesurés différents dans la fenêtre (relevé décrit plus bas), pas des champs entiers : un firmware qui montre un autre bit échoue jusqu'à ce qu'une ligne l'admette avec sa preuve.
  - Ligne 18 (APB_SARADC) : les sept registres de la source d'entropie, pas un de plus ; `SAR1DATA_STATUS` termine la fenêtre (test).
  - Ligne 9 (RTC_CNTL) : `RESET_STATE` et `OPTIONS0` sont comparés sauf leurs bits mesurés différents (bits 12–13 et `0x1C00A000`) ; la cause du reset est comparée. `TIME_LOW0` est copié en entier mais QEMU doit y lire 0 (son RTC_CNTL ne modélise que `OPTIONS0`, `RESET_STATE`, `STORE0`–`7`).
  - Ligne 10 (eFuse) : cinq mots de données, pas les registres du contrôleur.
  - Ligne 7 (SPI1) : `W0` n'est copié qu'après la lecture SFDP (`0x5A`, identifiée par l'opcode de `USER2` au rangement de `CMD.USR`) et après `RDSR` (`0x05`), où seuls les octets hauts restés périmés dans QEMU diffèrent ; la mesure trouve un chargement après chacune.
- **Paires au lieu de masques (R16).** Lignes 1 à 3 : des valeurs exactes de chaque côté (`mstatus.MPP` 0 contre `0x1800` ; `mie` `0x0EEE` contre 0 et `mtvec.MODE` 0 contre 1, jusqu'à la première écriture du CSR). Ligne 4 : le `MEIP` de QEMU doit égaler la ligne que sa matrice calcule (`esp32c3_intmatrix_line_should_assert`) dans l'état du contrôleur d'inkemu (`Esp32C3::external_interrupt_as_qemu`) ; mesure : égalité dans 91 158 comparaisons, et 3 336 = 3 × 1 112 où QEMU garde `MEIP` les trois instructions après le rangement du seuil par `_interrupt_handler` (pc `0x40380252`). Le mécanisme de ce retard dans QEMU n'est pas identifié.
- **Compteurs (R17).** Plus de copie de valeur entière. Le SYSTIMER de QEMU tronque à chaque mise à jour, met à jour un compteur arrêté en reprogrammant un comparateur, ignore `SYSTEM.PERIP_RST_EN0` et lit toujours `VALUE_VALID` : avant le réglage `systimer_counts_as_qemu`, QEMU avait 293 412 à 303 246 unités d'avance (19 ms) et l'écart dérivait d'une unité par tick. Avec lui, le compteur 1 est égal sur 2 200 lectures, le compteur 0 en avance d'une ou deux unités sur 14 lectures : borne 2. Le compteur de cycles de QEMU (6,25 ns de son horloge depuis la première lecture, reste reporté, `csrw` sans lecture) est reproduit par `cycle_counter_as_qemu` ; écart −1/0/+1 : borne 1. Sources : `hw/timer/esp_systimer.c` et `target/riscv/esp_cpu.c` de la branche `esp-develop` d'`espressif/qemu`, lus le 2026-10-14. L'égalité exacte demanderait de dater le temps d'inkemu à la nanoseconde comme QEMU (8 ns par instruction) et de reproduire l'arrondi à la microseconde de ses alarmes : le résidu est borné, sourcé, compté.
- **Fenêtre (R18).** Les périphériques de QEMU sont pris à l'octet dans `info mtree -f` (relevé le 2026-10-14) ; tout accès ailleurs dans `0x6000_0000`–`0x600F_FFFF` termine la fenêtre sauf registre nommé par une ligne : FE, FE2, NRX (absents du SVD), RTC_CNTL au-delà de `0x1FF`, SYSTEM au-delà de `0x9B` compris. La ligne 13 nomme les quatre registres de la radio que touche `rtc_sleep_pu`. Un reset d'un seul côté est une divergence (rapport « QEMU reset after this instruction and inkemu did not »).
- **Usage vérifié (R19).** `lockstep::Usage` enregistre ce que chaque ligne a fait ; `Usage::check` échoue sur une ligne inutilisée, une ligne inconnue, une prise hors des registres ou des bits déclarés, un écart hors borne (tests unitaires des quatre cas). Une ligne (iv) ne compte que les actions de son réglage, que la machine compte (`CompatActs` : alarmes retenues, resets ignorés, valeurs rendues autrement, lectures du compteur), y compris à travers un reset.
- **Doublons (R20).** Au plus un bloc réimprimé de suite (mesure : jamais deux) ; un saut sur lui-même est reconnu sur les bits de l'instruction et l'état de QEMU (`loops_on_itself`), plus sur le décodeur d'inkemu.
- **Machine et outil (R21).** `step` reprend à sa frontière après `Asleep` (test : alarme à 1,5 s) ; sous `qemu-icount`, une faute d'accès mémoire ne coûte rien, comme QEMU la rend à son compteur (`cpu_restore_state_from_tb`), un `ecall` coûte 8 ns (test) ; latence multipliée avec saturation ; test d'équivalence `run_within`/`step` sur deux programmes (interruption logicielle ; alarme et `wfi`) sous les deux modèles de temps. Outil : Ctrl-C → 130 (vérifié en vivant, conteneur supprimé), `Drop` du conteneur et du répertoire, Docker dans son groupe de processus, `--pull=never`, `--mount` qui refuse une virgule ou un chemin non UTF-8, 120 s pour que QEMU ouvre sa trace, trace sans bloc → 2 (vérifié), QEMU qui s'arrête → 2, flash de taille autre que le profil → 2 (vérifié), mode QEMU sous Unix seulement (`cargo check --target x86_64-pc-windows-gnu` passe), image et build de QEMU dans `versions.env` (`QEMU_ESP_BUILD`, test), `timeout-minutes` sur les étapes.

## Usage des lignes dans la fenêtre

Imprimé par l'outil à chaque exécution, identique en vivant et sur la trace capturée :

| Ligne | Usages | Détail |
|---|---|---|
| 1 | 2 781 658 | `mstatus`, bits `0x1800` |
| 2 | 3 402 170 | `mie`, bits `0x0EEE` |
| 3 | 15 | `mtvec`, bit 0 |
| 4 | 94 494 | `mip`, bit 11 |
| 5 | 532 149 | 283 221 lectures rendues comme QEMU ; `0x7E2` 1 144 valeurs, `0x802` 247 784, écart 1 |
| 6 | 3 165 | `0x6000001C` ×3 155, `0x60000020` ×1, `0x6000006C` ×5, `0x60000078` ×4 |
| 7 | 20 | `0x60002034` ×4, `0x60002058` ×2, `0x60002098` ×4, `0x6000209C` ×9, `0x600020DC` ×1 |
| 8 | 81 | 13 registres de SPI0 (loads et stores admis) |
| 9 | 92 | 20 registres de RTC_CNTL, dont `RESET_STATE` ×20 et `TIME_LOW0` ×5 |
| 10 | 67 | 5 mots d'eFuse |
| 11 | 284 | 22 broches d'IO_MUX |
| 12 | 147 | `0x6000E000` ×78, `0x6000E044` ×17, `0x6000E048` ×52 |
| 13 | 14 | FE2 ×2, FE ×2, NRX ×6, BB ×4 (rien copié) |
| 14 | 13 | 9 registres de TIMG0 et TIMG1 |
| 15 | 2 228 | 2 214 actions du réglage ; `0x60023044` ×14, écart 2 |
| 16 | 21 | 4 registres d'APB_CTRL |
| 17 | 1 604 | `RNG.DATA` |
| 18 | 48 | les 7 registres d'APB_SARADC |
| 19 | 7 033 | `EP1_CONF` bit 1 |
| 20 | 46 | 12 registres de SYSTEM |
| 21 | 77 | 20 registres de SENSITIVE |
| 22 | 24 | 6 registres d'EXTMEM |
| 23 | 14 447 | 7 registres d'ASSIST_DEBUG |
| 24 | 1 | une alarme retenue |

## Comment les lignes ont été mesurées

Un relevé temporaire (jamais versionné) a fait tourner la comparaison sur une trace de QEMU capturée avec `sleep=off`, en notant pour chaque adresse de périphérique lue ou écrite : le nombre de chargements et de rangements, l'union des bits différents entre QEMU et inkemu, les PC, les écarts, la largeur des accès (tous des mots), et, pour les compteurs, la suite des écarts dans le temps. Les masques du tableau sont ces unions ; chaque registre a été classé selon les périphériques de QEMU ; le contexte des chargements de `W0` (les rangements de `USER2` et `CMD` qui précèdent) et du retard de `MEIP` (les rangements qui précèdent) a été relevé de même. Les sources de QEMU ont été lues dans la branche `esp-develop` d'`espressif/qemu`. Pour remesurer, refaire un tel relevé : l'outil lui-même imprime déjà l'usage de chaque ligne, registre par registre.

## Décisions de ce plan (R1–R21)

Détaillées dans le plan ; les plus engageantes :

- **R1** l'unité est la frontière d'instruction : `Esp32C3::step` exécute une instruction et rend la machine à la frontière de la suivante (interruption prise, `wfi` traversé), y compris à travers un arrêt `Asleep`.
- **R3** deux sortes de blocs écartés : réimprimés (au plus un de suite) et **préemptés** — QEMU imprime un bloc en y entrant, puis prend l'interruption avant d'exécuter l'instruction ; le `-d int` qui suit a pour `epc` le PC du bloc (10 sur 1 112 interruptions).
- **R4** `qemu-icount` : 32 cycles de la base de temps toutes les 25 instructions ; interruptions, attentes et fautes d'accès gratuites ; CPI 1.
- **R5** `-icount shift=3,sleep=off` : avec `sleep=on`, deux exécutions de QEMU différaient de 21 instructions.
- **R6** remplacée par R18 (fenêtre au registre nommé) et la correction de la spec.
- **R15–R21** les correctifs de la revue, ci-dessus.

## Ce que le lockstep a tranché, et ce qu'il a seulement admis

- **Rien n'y diverge hors des registres nommés** : le cœur, le décodage, les CSR, les exceptions, la prise d'interruption (ligne, `mepc`, `mcause`, instant) et la lecture de la flash à travers la MMU et le cache sont identiques à QEMU sur toute la fenêtre ; la cause du reset (`RESET_STATE`) et les bits non mesurés différents de chaque registre copié le sont aussi.
- **Admises, pas tranchées** (QEMU n'est pas une référence pour elles) : les bits copiés des lignes 6 à 23. Chaque section du tableau dit ce que la copie laisse dans inkemu : le bootloader qui croit la PLL déjà sélectionnée (ligne 20), le moniteur de pile désactivé après la première interruption (ligne 23), la ROM qui renonce à attendre la console USB (ligne 19), le temporisateur RTC non comparé (ligne 9).
- **Hypothèse du SYSTIMER d'inkemu** : un reset de `SYSTEM.PERIP_RST_EN0` remet ses compteurs à 0 (TRM §10.3, marqué ASSUMPTION) ; QEMU ne le fait pas, ce qui ne tranche rien.

## Laissé aux héritiers

- **SP1b-4d** :
  - traverser le reset : QEMU réinitialise toute la machine, inkemu seulement le CPU ; repartir d'un état commun après le reset ;
  - CrossPoint, en local et daté ; sa fenêtre s'arrêtera au premier accès à un registre que QEMU n'a pas et qu'aucune ligne ne nomme (une mesure de l'ADC, SPI2, I2C0), ou demandera des lignes mesurées ;
  - la build QEMU `20260417` (interruptions perdues avec `MIE` = 0 corrigées) : rien dans la fenêtre de `hello_world` n'en a eu besoin ;
  - le mécanisme du retard de `MEIP` dans QEMU, à lire dans `target/riscv/cpu_helper.c` si un autre firmware montre un autre retard.
- **Sous-projet 2 / 4** : `Esp32C3::step` (avec sa frontière en attente), `x`, `set_x`, `csr` et `Hart::take_boundary` sont l'API du débogueur et du serveur GDB.
- **Sous-projet 8** : chaque ligne (ii) est une question pour un appareil (valeur de reset, ce que lit la puce).

## Limites de preuve

- Vérifié en local : `cargo fmt`, clippy (`-D warnings`) et rustdoc (`-D warnings`) sur `cpu-rv32`, `soc-esp32c3`, `lockstep`, `inkemu-lockstep` ; les tests de `cpu-rv32` (bibliothèque), `soc-esp32c3` (bibliothèque et tests d'intégration non ignorés), `lockstep` et `inkemu-lockstep` ; la compilation WebAssembly de `lockstep`, `soc-esp32c3` et `cpu-rv32` ; `cargo check --target x86_64-pc-windows-gnu -p inkemu-lockstep` ; l'empreinte de trace de `inkemu run` 12 s sur `flash_hello.bin` inchangée (`a0d221296337b295`) ; contre QEMU dans Docker : la comparaison complète (code 0, 24 lignes), la comparaison avec faute (code 1 à l'instruction 1 000 000), Ctrl-C (code 130, conteneur supprimé), et deux erreurs (trace qui n'est pas de QEMU, flash trop courte : code 2).
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace`, les suites ignorées (vraie ROM, firmwares de référence et leurs empreintes de trace épinglées), la couverture, les deux étapes du lockstep dans la CI elle-même.
- Le flot de la fenêtre ne dépend pas de l'aléa de QEMU (`srand(time(NULL))`, qu'aucune option ne fixe) d'après les comptes identiques de plusieurs exécutions ; rien ne le garantit pour un autre firmware.
- Le test d'équivalence `run_within`/`step` ne couvre pas un programme de sommeil profond.
