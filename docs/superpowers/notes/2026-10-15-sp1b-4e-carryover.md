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
| Exécution | `--max-time 20` : 2 792 650 730 cycles, 2 208 654 520 instructions (1,264 cycle par instruction), aucun avertissement, `trace-hash bfb20978732b673f`, 87 s pour le test (110 s pour l'exécution seule) |

- Aucun coût du modèle de temps n'a été touché ; `is_calibrated()` reste `false` (ADR 0009). La bande borne le coût
  moyen des instructions de CoreMark entre 0,966 et 1,307 cycle ; seule la borne basse est atteignable par un cœur
  à une instruction par cycle (un modèle à un cycle partout donnerait environ 537).
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
| Le premier tick, deux instructions plus tard chez QEMU quand le CPU exécute | **Reporté**, à moitié sourcé | QEMU ancre l'alarme sur son horloge virtuelle lue pendant le rangement, qui le compte (`esp_systimer.c` 105–154, `icount-common.c` 71–126) ; inkemu au début : une instruction. L'autre n'est pas dans `esp_systimer.c` (10 ms exactes, pas d'arrondi au compte) ; piste non établie : `icount_round` après un saut d'horloge. La règle de SP1b-4d n'admet un réglage que pour un écart sourcé là, et l'ancrage d'inkemu est une hypothèse que seule la puce tranche. | Sous-projet 8 (banc) : mesurer sur une puce la latence du premier tick après l'activation du compteur ; puis, si l'écart restant avec QEMU est sourcé, un réglage (iv), et les fenêtres des applications composées. |
| Le `MEIP` de QEMU dans `usb_serial_jtag_echo` | **Sourcé** comme artefact du QEMU épinglé ; pas de ligne | L'USB-Serial-JTAG de QEMU n'a pas de sortie d'interruption (hypothèse de SP1b-4d réfutée) ; la matrice de l'étiquette garde un bit « en attente » collant (ici l'interruption logicielle, ligne 3, priorité 1, sous le seuil 2 que le gestionnaire vient de ranger) et le CPU laisse `MEIP` levé tant qu'elle en rapporte un. Une ligne appartient à une fenêtre qui l'utilise (R35) ; aucune application composée n'a de fenêtre avant le tick. | Avec le tick, au sous-projet 8 : le traitement (modéliser le bit en attente de QEMU sur l'état d'inkemu) entre avec la fenêtre d'`usb_serial_jtag_echo`. |
| — trouvé en chemin : la ligne 4 cite la branche `esp-develop`, pas l'étiquette épinglée | **Corrigé** (source seulement) | Le « retard de trois instructions, mécanisme non identifié » est le `MEIP` que le CPU de l'étiquette laisse levé depuis l'interruption prise jusqu'au `csrs mstatus`. Mesure, paramètres, usages et épingles inchangés. | — |

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
  premier tick ; puis les fenêtres des applications composées avec le traitement du bit en attente de QEMU. Si une
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
