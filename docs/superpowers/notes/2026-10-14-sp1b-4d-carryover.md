# SP1b-4d — Note de passation

Plan : `docs/superpowers/plans/2026-10-14-sp1b-4d-lockstep-application.md` (R30–R47 ; R42–R47 à la relecture). Spec : `docs/superpowers/specs/2026-10-14-sp1b-4d-design.md` (J1–J9, et sa note du 2026-10-14 après les relectures). ADR 0008, sections « Révision du 2026-10-14, SP1b-4d » et « … relectures de SP1b-4d ». Tableau : `docs/lockstep-differences.md` (lignes 25 à 41, section « Les fenêtres »).

## Résultat

Mesures du 2026-10-14, QEMU `esp_develop_9.2.2_20250817` dans `espressif/idf:v5.5.2`, un conteneur à la fois :

| Fenêtre | Où | Comparées | Fin | Interruptions | Réimprimés | Préemptés | Plus grand mouvement |
|---|---|---|---|---|---|---|---|
| hello | CI | 6 813 949 | reset, 1 traversé | 2 224 | 100 | 20 | 1 (lignes 5 et 15) |
| nvs | CI | 7 680 555 | reset, 1 traversé | 2 224 | 114 | 20 | 1 (lignes 5 et 15) |
| nvs-ota | CI | 7 701 890 | reset, 1 traversé | 2 224 | 118 | 20 | 1 (lignes 5 et 15) |
| crosspoint | local | 31 088 420 | `APB_SARADC.APB_TSENS_CTRL` (`0x60040058`), PC `0x4214917C` | 0 | 657 | 0 | 1 (lignes 5 et 39) |

- La CI (job `reference-boot`) épingle le compte, la fin (`--expect-end reset`) et l'usage de chaque ligne des trois premières (`tools/ci/lockstep-{hello,nvs,nvs-ota}.usage`), et exige qu'une faute dans `t0` mise après le reset (`--perturb-after 4000000`) diverge à l'instruction 4 000 000, un reset traversé. Mesuré en local : divergence à 4 000 000, « resets crossed: 1 ».
- **CrossPoint 1.6.5, 2026-10-14** : fichier de l'utilisateur, empreinte vérifiée (`8eccf614…0385764`), flash composée derrière le bootloader livré (asset public et octets effacés seulement, écrite par l'outil, réservée au propriétaire, dans son répertoire temporaire, jamais versionnée). Code de sortie 0 avec le compte, la fin et les usages épinglés dans `crates/lockstep-cli/tests/crosspoint.rs` et `crosspoint.usage`. La ROM, le bootloader (dont l'écriture d'`otadata`, vers 30,7 millions) et le début de l'application ; la fenêtre finit à la lecture du capteur de température, que QEMU n'a pas et qu'aucune ligne n'admet. L'objectif d'environ 32 millions de la consigne est atteint à 3 % près, par la fin naturelle de la fenêtre et non par une borne (R41). Durée : une vingtaine de minutes.
- 40 lignes et une retirée (la ligne 1, `mstatus.MPP`, dont l'écart était un défaut d'inkemu) : 23 de SP1b-4c, 17 nouvelles. Trois traitements nouveaux, tous liés au reset : gardé au reset (paire tenue dans ses bits mesurés jusqu'à l'écriture, depuis la valeur de reset déclarée ; lecture = divergence), CSR gardé au reset (PMP, jusqu'à une écriture entière), copie après reset (jusqu'au premier rangement ; aussi après un reset de module que QEMU ignore). Aucune resynchronisation, aucun nouveau réglage `QemuCompat`.
- La machine par défaut change en un point, à la relecture : `mstatus` du cœur de l'ESP32-C3 (reset 0, `MPP` dans {0, 3}, TRM §1.4.2 et registre 1.5), dans `cpu-rv32`, avec ses tests. Les empreintes de trace épinglées sont inchangées, revérifiées : 12 s `a0d221296337b295`, 40 000 001 cycles `fb9ea628bc25b355`, `adc_wait_panic` `b217bc3b287080ae`, `deep_sleep_timer` `80bd1c43d55b5467`. `soc-esp32c3` ne gagne qu'un commentaire `ASSUMPTION:` (l'ancrage de l'alarme du SYSTIMER).

## Ronde de relecture (2026-10-14, P1–P9 et M1–M9 ; plan R42–R47)

- **`mstatus`** (M3) : corrigé dans `cpu-rv32` d'après le TRM (§1.4.2, registre 1.5) ; ligne 1 retirée, numéro gardé. Ancienne valeur au reset et après `mret` : `0x1800` ; nouvelle : 0. Empreintes de trace inchangées.
- **Tenues bornées** (P1–P3) : bits mesurés et valeur de reset déclarée ; CSR jusqu'à une écriture entière (le `csrrs` du bootloader sur `pmpcfg0` porte la différence qu'il a lue) ; copies après reset jusqu'au premier rangement, ré-armées par une impulsion de module que QEMU ignore. Relevé de chaque chargement et rangement des registres de la ligne 27 dans la seconde vie de `hello` : une différence après un rangement suit toujours une impulsion du module (UART0, SPI0/1), sauf `0x600080AC` bit 31 et `PERIP_RST_EN0/1`, que QEMU ne stocke pas : passés aux lignes 9 et 20.
- **SPI1 après le reset** (P4, M9) : relevé de la seconde vie, inkemu est aux valeurs du SVD (`USER` `0x80000000`, `USER2` `0x70000000`, `CTRL2` et `MISO_DLEN` 0) ; l'écart est celui du reset de QEMU (`esp32c3_spi_reset_hold`). Aucun défaut d'inkemu.
- **Fenêtres remesurées** sur des traces capturées à nouveau : `hello` 6 813 949, `nvs` 7 680 555, `nvs-ota` 7 701 890, `crosspoint` 31 088 420 ; fins inchangées ; usages : ligne 1 retirée, ligne 9 165 (162), ligne 27 46 (51) dans les trois fenêtres de la CI, `crosspoint` inchangée hors ligne 1. Faute du SYSTIMER après le reset (`--perturb-systimer-after 5000000`) : divergence à l'instruction 5 700 683, ligne la plus proche 15, un reset traversé ; épinglée en CI.
- **CrossPoint, 2026-10-14, après la relecture** : sur la trace capturée, code 0 avec le compte, la fin et les usages épinglés ; puis le test local lui-même (`cargo test --release -p inkemu-lockstep --test crosspoint -- --ignored`), QEMU vivant : passé en 1 189 s. Un premier essai vivant avait échoué parce que Docker avait mis plus de 120 s à démarrer le conteneur (233 s à 272 s mesurés ce jour) : l'outil lui en donne désormais 300.

## Le reset

- **inkemu est juste** (R30) : `esp_restart` → `esp_rom_software_reset_cpu(0)` (lu dans `system_internal.c` de l'image épinglée) → `SW_PROCPU_RST`, cause `RTC_SW_CPU_RST` (0x0C), reset du CPU (TRM §6.1). QEMU donne la même cause. Rien n'a été corrigé dans inkemu.
- **Ce que QEMU garde à son reset**, mesuré : ses registres entiers, `mtvec`, `mepc`, `mstatus.MPIE` ; les bits R, W, X de la PMP (`0x89888F88` avant, `0x01000700` lu par QEMU, 0 par inkemu) ; le `CLKDIV` de l'UART (`0x30015B` gardé par QEMU, qui ignore l'impulsion `SYSTEM_UART_RST` ; `0x2B6` chez inkemu, qui remet UART0 à zéro) ; les registres de SPI1, que le reset de QEMU ne remet pas entièrement (`esp32c3_spi_reset_hold`) ; le `WEL` de sa flash après un programme (lignes 29, 33, 34 ; `esp-idf-storage-flash-traffic.md`, résultat clé 5). Aucune instruction des fenêtres ne calcule avec une valeur gardée : la règle la plus stricte passe.

## Les firmwares sans fenêtre (R37)

| Firmware | Arrêt | Observation (cause non établie) |
|---|---|---|
| `adc_wait_panic` | 3 986 033 | premier tick de FreeRTOS pendant que le CPU exécute : QEMU le prend deux instructions après inkemu |
| `uart_select` | 3 963 178 | idem (rangement d'activation du compteur 1 à l'instruction 2 713 179 ; `epc` de QEMU `0x420092A2`, d'inkemu `0x4200929E`) |
| `usb_serial_jtag_console` | 3 848 562 | idem |
| `deep_sleep_timer` | 3 921 093 | idem |
| `usb_serial_jtag_echo` | 2 525 511 | après que le gestionnaire rouvre `MIE` (`csrs mstatus` en `0x40380260`), QEMU montre `MEIP` alors que sa matrice, calculée sur l'état du contrôleur d'inkemu, ne lève pas la ligne |

Le tick : inkemu ancre l'alarme périodique au début du rangement qui active le compteur (une hypothèse, marquée `ASSUMPTION:` dans `Systimer::write_conf`), et la lève après l'instruction s + 1 249 999 ; QEMU après s + 1 250 001. Un ancrage à la fin du rangement expliquerait une des deux instructions ; la cause de l'autre n'est pas établie. Sur la puce, le rangement traverse l'APB et le domaine d'horloge à 16 MHz du timer, ce qui ne peut que le retarder : inkemu serait plutôt en avance. Le lockstep tourne aussi avec une latence d'interruption de 0 sous `qemu-icount`, là où le TRM donne 4 cycles. L'écart n'est donc pas attribué à QEMU. Pour `usb_serial_jtag_echo`, la ligne de QEMU est calculée sur l'état du contrôleur et des sources d'inkemu, et les deux modèles de l'USB-Serial-JTAG diffèrent (ligne 19) : une source haute chez QEMU et basse chez inkemu donnerait la même observation. Dans `hello` et `nvs`, chaque tick tombe dans un `wfi`, où les deux instructions n'existent pas. Ni l'un ni l'autre n'est admis : ce serait une ligne sur l'instant d'une interruption, que le lockstep doit prouver.

## Comment les lignes ont été mesurées

Comme pour SP1b-4c (R40) : traces de QEMU capturées compressées, comparées hors ligne autant que nécessaire ; un relevé temporaire, jamais versionné, a noté chaque chargement différent, par vie et par registre ; les masques sont ces unions. L'outil imprime ensuite ce que chaque ligne a fait, et la CI l'épingle.

## Décisions (R30–R41)

Détaillées dans le plan ; les plus engageantes : R31 (la traversée ne touche rien), R32 (ce que QEMU garde est une paire tenue, pas une copie), R35 (une fenêtre a ses lignes ; un autre firmware reçoit ses propres lignes plutôt qu'un usage partiel), R37 (les applications composées n'ont pas de fenêtre), R38 (pas de changement de QEMU : `20260417` corrige des interruptions perdues avec `MIE` = 0, qu'aucune fenêtre ne rencontre), R41 (CrossPoint finit où QEMU s'arrête).

## Laissé aux héritiers

- **SP1b-4e et suivants** : l'écart de deux instructions du premier tick quand le CPU exécute, cause non établie. Trancher d'abord l'instant d'inkemu (ancrage de l'alarme au début ou à la fin du rangement de `CONF`, hypothèse de `Systimer::write_conf` ; latence d'interruption sous `qemu-icount`), sur la puce si possible ; un réglage (iv) seulement si l'écart restant est sourcé dans `esp_systimer.c`. Tant que ce n'est pas tranché, aucune application composée n'a de fenêtre.
- **SP1b-4f** : l'exécution en mode utilisateur. `mstatus.MPP` vaut désormais 0 après un `mret` comme sur la puce, mais un `mret` vers le mode utilisateur continue en mode machine (accès aux CSR et PMP non vérifiés) ; aucun firmware mesuré ne le fait.
- Le `MEIP` de QEMU dans le chemin USB-Serial-JTAG (`usb_serial_jtag_echo`) : cause non identifiée, à lire dans `hw/riscv/esp32c3_intmatrix.c` et le modèle USB-Serial-JTAG de QEMU.
- CrossPoint au-delà du capteur de température : QEMU n'a pas de TSENS ; aller plus loin demande un oracle qui l'a (le banc du sous-projet 8).
- **Sous-projet 8** (questions pour un appareil) :
  - les registres entiers et `mepc` après un reset du CPU (hypothèses de `Hart::reset`, rapport IR §4.2), et l'effacement des bits L de la PMP ;
  - ce que remet à zéro le bit 0 de `PERIP_RST_EN0` (`SYSTEM_TIMERS_RST`), qu'`esp_restart` pulse et qu'inkemu suppose sans effet : si la puce remet TIMG0 et TIMG1 à zéro, quatre registres TIMG de la ligne 27 chargent QEMU d'un défaut d'inkemu (rapport IR §4.3 et §7 question 1) ;
  - l'instant du premier tick après le lancement du comparateur (l'ancrage de l'alarme du SYSTIMER) ;
  - la valeur de `MPIE` après un `mret` (le TRM dit « effacé », QEMU et inkemu le posent) ;
  - le `CLKDIV` de l'UART après un reset du CPU, et chaque ligne (ii).

## Limites de preuve

- Vérifié en local : `cargo fmt --all --check` ; clippy et rustdoc (`-D warnings`) sur `lockstep` et `inkemu-lockstep` ; `cargo test -p lockstep -p inkemu-lockstep` ; la compilation WebAssembly de `lockstep` ; l'empreinte de 12 s ; contre QEMU dans Docker, les trois fenêtres de la CI, la faute après le reset, les cinq applications composées jusqu'à leur arrêt, et CrossPoint une fois.
- Vérifié en local à la relecture : `cargo fmt --all --check` ; clippy et rustdoc (`-D warnings`) sur `cpu-rv32`, `soc-esp32c3`, `lockstep`, `inkemu-lockstep` ; les tests de `cpu-rv32` (bibliothèque et intégration), `lockstep`, `inkemu-lockstep` ; WebAssembly de `lockstep` et `cpu-rv32` ; les quatre empreintes de trace citées plus haut ; sur des traces de QEMU capturées à nouveau, les trois fenêtres de la CI, les fautes de `t0` et du SYSTIMER après le reset, CrossPoint ; et le test local de CrossPoint avec QEMU vivant. Les applications composées n'ont pas été remesurées (leur arrêt tient à l'instant d'une interruption, pas aux lignes ni à `mstatus`).
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace`, les suites ignorées, la couverture, les étapes du lockstep dans la CI elle-même.
- **Jamais exécuté dans la CI** : la fenêtre `crosspoint` (le fichier est celui de l'utilisateur). Son résultat est celui du 2026-10-14 ; une autre build de QEMU ou une autre révision d'inkemu demandent de le refaire.
- La longueur de chaque fenêtre ne dépend pas de l'aléa de QEMU d'après les mesures répétées ; rien ne le garantit pour un autre firmware (ligne 17).
