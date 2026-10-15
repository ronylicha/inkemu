# SP1b-4e — Note de passation

Plan : `docs/superpowers/plans/2026-10-15-sp1b-4e-coremark.md` (R55–R64). Spec :
`docs/superpowers/specs/2026-10-15-sp1b-4e-design.md` (K1–K10). Recherche : `docs/research/coremark-esp32c3.md`.
ADR 0009 (nouvel) ; ADR 0008, section « Révision du 2026-10-15, SP1b-4e » ; `docs/lockstep-differences.md`, deux
révisions datées (paragraphe des applications composées, ligne 4).

## Résultat

Critère 5 de la spec (§10.3), mesuré le 2026-10-15 sur le poste de développement, release de cette branche, ROM rev3 :

| Grandeur | Valeur |
|---|---|
| Firmware | `coremark_example.bin` (`espressif/coremark` 1.1.0~2, sources copiées sans modification, GCC 14.2.0, `-O3 -fjump-tables -ftree-switch-conversion`, code en IRAM), SHA-256 `1de13ef8c2640668640eb52a12f6b015f9efbdd55d3c28109ae640c8f42fbdbf` |
| Score imprimé | **424,628450** : 6000 itérations en 14 130 ms d'`esp_timer`, c'est-à-dire du temps d'appareil d'inkemu |
| Référence (K1) | 483,27, fiche technique v2.4 page 3 ; bande [410,78 ; 555,76] ; écart **−12,1 %** |
| Autre chiffre publié | 409,25 (exemple, GCC 12.2) : +3,8 %, rapporté, pas testé |
| Validité CoreMark | CRC de la course de performance 2K (seedcrc 0xe9f5, list 0xe714, matrix 0x1fd7, state 0x8e3a, final 0xa14c), « Correct operation validated », plus de 10 s |
| Exécution | `--max-time 20` : 2 792 650 730 cycles, 2 208 654 520 instructions (1,264 cycle par instruction), aucun avertissement, `CPU clock at 160 MHz`, `trace-hash bfb20978732b673f` |
| Durées mesurées | test : **68 s en CI** (job `reference-boot` de la PR #14, « finished in 68.26s »), 87 s, 102 s et 328 s sur le poste de développement, partagé avec d'autres travaux (trois passages ; le dernier sous une charge moyenne d'environ 17 sur 16 cœurs) |

- Aucun coût du modèle de temps n'a été touché ; `is_calibrated()` reste `false` (ADR 0009). La bande borne le coût
  moyen des instructions de CoreMark entre 0,966 et 1,307 cycle. Le test vérifie les deux bornes, mais la haute est
  hors d'atteinte d'un modèle qui coûte au moins un cycle par instruction (un cycle partout donne environ 537).
- Le test échoue hors de la bande : coût ALU passé à 2 cycles, score 295,53 (−38,8 %), échec sur la bande ; défait.
- La construction : deux fois depuis zéro (à la main, puis `build.sh` avec tous les firmwares de référence), mêmes
  empreintes ; les 20 fichiers copiés vérifiés contre le `CHECKSUMS.json` du registre, puis par `in-container.sh`
  avant chaque construction.
- CI : job `reference-boot`, variable `INKEMU_COREMARK_APP`, effectif `run_cli reference_firmware` 13 → 14 (environ
  deux minutes de plus).
- Inchangés, revérifiés : l'empreinte de 12 s `a0d221296337b295`, le test de la trace épinglée (40 000 001 cycles
  `fb9ea628bc25b355`), le test du README sur les images de référence ; `cargo test -p lockstep` (le tableau des
  différences, ses révisions comprises). `adc_wait_panic` et `deep_sleep_timer` n'ont pas été relancés (aucun code de
  la machine n'a changé ; la CI les tient).

## Les reports de SP1b-4d, tranchés

| Report | Décision | Raison | Héritier |
|---|---|---|---|
| Le premier tick, deux instructions plus tard chez QEMU quand le CPU exécute | **Reporté** ; une instruction expliquée par un raisonnement sur le code, non mesuré | QEMU ancre l'alarme sur son horloge virtuelle lue pendant le rangement, que le décompte sous `-icount` fait compter (`esp_systimer.c` 105–154, `icount-common.c` 71–126 ; le « compte » est un raisonnement) ; inkemu au début : une instruction. L'autre n'est pas dans `esp_systimer.c` (10 ms exactes, pas d'arrondi au compte) ; piste non établie : `icount_round` après un saut d'horloge. La règle de SP1b-4d n'admet un réglage que pour un écart sourcé là, et l'ancrage d'inkemu est une hypothèse que seule la puce tranche. | Sous-projet 8 (banc) : mesurer sur une puce la latence du premier tick après l'activation du compteur ; puis, si l'écart restant avec QEMU est sourcé, un réglage (iv), et les fenêtres des applications composées. |
| Le `MEIP` de QEMU dans `usb_serial_jtag_echo` | **Hypothèse de SP1b-4d réfutée** (seul fait établi) ; **cause déduite du code de QEMU et de l'état à l'arrêt, non observée dans QEMU** ; pas de ligne | Établi : l'USB-Serial-JTAG de QEMU n'a pas de sortie d'interruption. Déduit : la matrice de l'étiquette garderait un bit « en attente » collant (ici l'interruption logicielle, ligne 3, priorité 1, sous le seuil 2 que le gestionnaire vient de ranger) et le CPU laisserait `MEIP` levé tant qu'elle en rapporte un. Une ligne appartient à une fenêtre qui l'utilise (R35) ; aucune application composée n'a de fenêtre avant le tick. | Avec le tick, au sous-projet 8 : d'abord **observer le bit dans QEMU** (moniteur ou gdbstub), puis le traitement (modéliser le bit en attente de QEMU sur l'état d'inkemu) avec la fenêtre d'`usb_serial_jtag_echo`. |
| — trouvé en chemin : la ligne 4 cite la branche `esp-develop`, pas l'étiquette épinglée | **Corrigé** (source seulement) | Le « retard de trois instructions, mécanisme non identifié » s'explique, à la lecture du code de l'étiquette, par le `MEIP` que son CPU laisse levé depuis l'interruption prise jusqu'au `csrs mstatus` (déduit, non observé). Mesure, paramètres, usages et épingles inchangés. | — |
| — à la relecture : les sources du tableau citées sur la branche `esp-develop` | **Relues contre l'étiquette** | L'image ne contient que le binaire (`--version` : `esp_develop_9.2.2_20250817`), les sources sont lues à l'étiquette. Lignes 9, 17, 20, 27, 28, 30 (et 32, 37) : fichiers identiques ; 15, 24 (et 39) : `esp_systimer.c` ne diffère que par des traces de débogage ; 5 : fonctions identiques ; **2** : citait `esp_cpu_has_work` (branche) ; la valeur au reset vient de `esp32c3_intmatrix_reset_hold` dans l'étiquette, source corrigée ; 4 : ci-dessus. Aucune mesure ni épingle ne change. Aucune ligne n'est restée sans relecture. | — |

Mesure du `MEIP`, 2026-10-15 : une fenêtre de mesure temporaire (toute la table, jamais versionnée) sur
l'application composée : arrêt à 2 525 511 instructions comparées (le compte de SP1b-4d), `csrs mstatus` en
`0x4038025c`, `mcause` 0x80000003, `mip` QEMU 0x800, inkemu 0, pas de piège ; un relevé temporaire des écritures de la
matrice d'inkemu (jamais versionné) donne la ligne 3 = `FROM_CPU_INTR0` (source 50), priorité 1. Les deux
modifications temporaires ont été retirées avant tout commit.

## Décisions prises sans l'utilisateur

K1–K10 de la spec et R55–R64 du plan ; les plus engageantes : la référence est la fiche technique (483,27), le chiffre
de l'exemple seulement rapporté (R55) ; aucun coût ajusté, le modèle reste « non calibré » (R56, ADR 0009) ; les
sources de CoreMark copiées dans le dépôt plutôt que téléchargées par la CI (R57) ; le `NOTICE` racine inchangé
(R60) ; le tick et le traitement du `MEIP` reportés au sous-projet 8 (R61, R62).

## Laissé aux héritiers

- **Sous-projet 8** : l'ajustement des coûts sur une puce (CoreMark n'en contraint que la moyenne) ; l'instant du
  premier tick ; **l'observation dans QEMU (moniteur ou gdbstub) du bit en attente que la relecture du code
  suppose dans `usb_serial_jtag_echo`** ; puis les fenêtres des applications composées avec le traitement de ce
  bit. Si une
  puce donne un score éloigné de 483,27, une révision de l'ADR 0009 change la référence, avec sa raison.
- **SP1b-4f** : la PMP (inchangé).
- Le compte d'instructions par itération contre QEMU n'est pas fait (R64) : non demandé, des heures de lockstep.

## Limites de preuve

- **Vérifié en local** : `cargo fmt --all --check` ; `tools/ci/clippy.sh` (`-D warnings`) sur `emu-cli` et
  `soc-esp32c3` ; rustdoc (`-D warnings`) sur `soc-esp32c3` ; `cargo test -p lockstep` ; le test de CoreMark (passé)
  et sa mutation (échouée, défaite) ; le test de la trace épinglée de `flash_hello.bin` ; le test du README sur les
  images de référence ; l'empreinte de 12 s ; `build.sh` entier dans `espressif/idf:v5.5.2` ; contre QEMU, la fenêtre
  de mesure de `usb_serial_jtag_echo` (un conteneur à la fois).
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace`, les autres suites ignorées (dont les 13 autres
  tests `reference_firmware` de `run_cli`, ceux de `reference_boot` et de `console-script`), la couverture, les
  étapes du lockstep.
- La cause du `MEIP` est lue dans le code de l'étiquette et dans l'état à l'arrêt, pas observée dans QEMU (aucune
  instrumentation de QEMU). La seconde instruction du tick n'est pas expliquée.
- Le score dépend de GCC 14.2 (image épinglée) : un autre compilateur change le nombre d'instructions par itération.

## Relecture (2026-10-15, PR #14 à `17a9867`, CI verte ; I1–I3, M1–M9)

| Constat | Correction |
|---|---|
| I1 « artefact de QEMU » présenté comme établi | Reformulé partout (ADR 0008, spec K8 b, ce document, tableau, rapport, plan R62) : seule la réfutation de l'hypothèse USB-Serial-JTAG est établie ; la cause est déduite du code de QEMU et de l'état à l'arrêt, non observée ; l'observation va au sous-projet 8. Aucune ligne ajoutée. |
| I2 sources du tableau sur la branche | Référence posée sur l'étiquette (tableau, révision datée ; ADR 0008) ; lignes 2, 9, 15, 17, 20, 24, 27, 28, 30 (et 5, 32, 37, 39) relues contre elle, voir le tableau des reports ci-dessus ; la ligne 2 seule change de source. |
| I3 `cpu-rv32/src/timing.rs` « fitted against CoreMark in SP1b » | Commentaires réécrits selon l'ADR 0009 (non calibré, ajusté seulement sur un appareil au sous-projet 8), par Edit ; aucune valeur changée ; spec K7 précisée. |
| M1 séquence vérifiée avant la bande | Le score est tenu à la bande dès que sa ligne est lue (`coremark_score_within_criterion_5`), puis figure dans chaque message d'échec. |
| M2 `crcfinal` seulement pour 6000 | Table `COREMARK_FINAL_CRCS` (6000 → 0xa14c, de sources autres qu'inkemu) ; tout autre nombre échoue, sans inviter à ré-épingler. Vérifié par mutation (6000 → 6001 : échec avec ce message ; défaite). |
| M3 160 MHz seulement du firmware | La ligne `CPU clock at 160 MHz` du résumé d'inkemu est vérifiée aussi. |
| M4 fichier ajouté non détecté | `in-container.sh` compare la liste des fichiers de `coremark_example/` à `SHA256SUMS.coremark-sources` (plus `PROVENANCE.md`) ; vérifié dans l'image épinglée : un fichier ajouté ou un fichier épinglé retiré arrête la construction. |
| M5 durées incohérentes | Durées mesurées partout : test 68 s en CI, 87 et 102 s sur le poste. Le chiffre d'une exécution seule (110 s, une mesure à l'horloge murale prise à un autre moment, sous une autre charge du poste) est retiré à la seconde relecture (N6) : plus long que le test qui la contient, il ne disait rien de comparable. |
| M6 extrait sans `Compiler flags` ni `CoreMark 1.0` | Les deux lignes ajoutées au rapport, les 35 options `-fdebug-prefix-map` abrégées et l'abréviation dite. |
| M7 « ne voit que la borne basse » | Le test vérifie les deux bornes ; la haute est hors d'atteinte d'un modèle à au moins un cycle par instruction (environ 537) : ADR 0009, rapport, ce document, commentaire du test. |
| M8 « une instruction sur deux sourcée » | « expliquée par un raisonnement sur le code, non mesuré » (ADR 0008, K8 a, tableau, rapport, plan R61). |
| M9 README sans compilateur | « GCC 14.2.0, CoreMark en `-O3`, code en IRAM » ajouté (EN, FR), d'après la sortie de la construction (`Compiler version`, `Compiler flags`, `Memory location`). |

Vérifié en local à la relecture : `cargo fmt --all --check` ; clippy (`-D warnings`) sur `emu-cli` et `cpu-rv32` ;
rustdoc sur `cpu-rv32` ; `cargo test -p cpu-rv32 --lib timing` ; `cargo test -p lockstep --test table` ; le test
CoreMark (passé en 102 s) et la mutation de M2 ; la vérification de liste de M4 dans `espressif/idf:v5.5.2` ; la
version du QEMU de l'image. Aucune mesure, aucune empreinte, aucune épingle ne change.

## Seconde relecture (2026-10-15, à `d464635` ; N1–N6, plan R73–R78)

| Constat | Correction |
|---|---|
| N1 le motif `; CPU clock at 160 MHz;` exigeait que le CPU dorme | Le test lit le résumé ligne à ligne : `; CPU clock at 160 MHz` suivi de `;` ou de la fin de la ligne ; le commentaire dit « horloge à la fin de l'exécution ». |
| N2 « admits, measured » dans `IntMatrix::asserted_as_qemu` | Le retard de trois instructions est mesuré ; qu'il soit là où les deux modèles diffèrent est déduit du code, non observé dans QEMU. |
| N3 « Datasheet-derived », « vient de documents » | « Estimated starting point » (`timing.rs`), « estimé, pas mesuré » (README EN, FR). Le nom `DatasheetTiming` et la ligne `datasheet-uncalibrated`, identifiants imprimés et épinglés, restent. Aucune valeur ne change. |
| N4 §5 du rapport | Aligné sur K5 : score d'abord, table itérations → `crcfinal`, horloge lue dans le résumé d'inkemu. |
| N5 `sha256sum` « s'arrête à la première différence » ; `find -type f` aveugle aux liens | Phrase corrigée (`sha256sum --quiet -c` rapporte toutes les différences) ; la liste est faite de chaque entrée avec son type (`find . ! -type d -printf '%y %P\n'`). Vérifié dans l'image : fichier ajouté, fichier épinglé retiré, lien ajouté, fichier épinglé remplacé par un lien vers une copie identique, tous arrêtent la construction ; deux fichiers modifiés sont tous deux rapportés. |
| N6 « exécution seule 110 s » plus longue que le test | Retiré (mesure prise à un autre moment, sous une autre charge). Les durées locales sont données avec leur dispersion : 87, 102 et 328 s (ce dernier sous une charge moyenne d'environ 17 sur 16 cœurs) ; celle de la CI, 68 s, sert de référence. |

Vérifié en local : `cargo fmt --all --check` ; clippy (`-D warnings`) sur `emu-cli`, `soc-esp32c3` et `cpu-rv32` ;
rustdoc sur `soc-esp32c3` et `cpu-rv32` ; le test CoreMark (passé, 328 s sous charge) ; les contrôles de liste et
d'empreintes de `in-container.sh` dans `espressif/idf:v5.5.2`. Aucune mesure, aucune empreinte, aucune épingle ne
change.
