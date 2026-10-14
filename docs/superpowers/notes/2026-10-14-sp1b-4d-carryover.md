# SP1b-4d — Note de passation

Plan : `docs/superpowers/plans/2026-10-14-sp1b-4d-lockstep-application.md` (R30–R41). Spec : `docs/superpowers/specs/2026-10-14-sp1b-4d-design.md` (J1–J9). ADR 0008, section « Révision du 2026-10-14, SP1b-4d ». Tableau : `docs/lockstep-differences.md` (lignes 25 à 41, section « Les fenêtres »).

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
- 41 lignes : 24 de SP1b-4c inchangées, 17 nouvelles. Trois traitements nouveaux, tous liés au reset : gardé au reset (paire tenue jusqu'à l'écriture, lecture = divergence), CSR gardé au reset (PMP), copie après reset. Aucune resynchronisation, aucun nouveau réglage `QemuCompat`.
- La machine par défaut ne change pas : rien dans `soc-esp32c3` ni `cpu-rv32` ; l'empreinte de 12 s de `flash_hello.bin` vérifiée, `a0d221296337b295`.

## Le reset

- **inkemu est juste** (R30) : `esp_restart` → `esp_rom_software_reset_cpu(0)` (lu dans `system_internal.c` de l'image épinglée) → `SW_PROCPU_RST`, cause `RTC_SW_CPU_RST` (0x0C), reset du CPU (TRM §6.1). QEMU donne la même cause. Rien n'a été corrigé dans inkemu.
- **Ce que QEMU garde à son reset**, mesuré : ses registres entiers, `mtvec`, `mepc`, `mstatus.MPIE` ; les bits R, W, X de la PMP (`0x89888F88` avant, `0x01000700` lu par QEMU, 0 par inkemu) ; le `CLKDIV` de l'UART (`0x30015B` gardé, `0x2B6` au reset chez inkemu) ; le `WEL` de sa flash après un programme (lignes 29, 33, 34 ; `esp-idf-storage-flash-traffic.md`, résultat clé 5). Aucune instruction des fenêtres ne calcule avec une valeur gardée : la règle la plus stricte passe.

## Les firmwares sans fenêtre (R37)

| Firmware | Arrêt | Cause mesurée |
|---|---|---|
| `adc_wait_panic` | 3 986 033 | premier tick de FreeRTOS pendant que le CPU exécute : QEMU le prend deux instructions après inkemu |
| `uart_select` | 3 963 178 | idem (rangement d'activation du compteur 1 à l'instruction 2 713 179 ; `epc` de QEMU `0x420092A2`, d'inkemu `0x4200929E`) |
| `usb_serial_jtag_console` | 3 848 562 | idem |
| `deep_sleep_timer` | 3 921 093 | idem |
| `usb_serial_jtag_echo` | 2 525 511 | après que le gestionnaire rouvre `MIE` (`csrs mstatus` en `0x40380260`), QEMU montre `MEIP` alors que sa matrice, calculée sur l'état du contrôleur d'inkemu, ne lève pas la ligne |

Le tick : inkemu ancre l'alarme périodique au début du rangement qui active le compteur, et la lève après l'instruction s + 1 249 999 ; QEMU après s + 1 250 001. Dans `hello` et `nvs`, chaque tick tombe dans un `wfi`, où les deux instructions n'existent pas. Ni l'un ni l'autre n'est admis : ce serait une ligne sur l'instant d'une interruption, que le lockstep doit prouver.

## Comment les lignes ont été mesurées

Comme pour SP1b-4c (R40) : traces de QEMU capturées compressées, comparées hors ligne autant que nécessaire ; un relevé temporaire, jamais versionné, a noté chaque chargement différent, par vie et par registre ; les masques sont ces unions. L'outil imprime ensuite ce que chaque ligne a fait, et la CI l'épingle.

## Décisions (R30–R41)

Détaillées dans le plan ; les plus engageantes : R31 (la traversée ne touche rien), R32 (ce que QEMU garde est une paire tenue, pas une copie), R35 (une fenêtre a ses lignes ; un autre firmware reçoit ses propres lignes plutôt qu'un usage partiel), R37 (les applications composées n'ont pas de fenêtre), R38 (pas de changement de QEMU : `20260417` corrige des interruptions perdues avec `MIE` = 0, qu'aucune fenêtre ne rencontre), R41 (CrossPoint finit où QEMU s'arrête).

## Laissé aux héritiers

- **SP1b-4e et suivants** : le retard de deux instructions du tick de QEMU quand le CPU exécute. Deux voies : un réglage (iv) mesuré et sourcé dans `esp_systimer.c` (instant d'armement de l'alarme), ou l'explication que l'instant d'inkemu doit suivre. Tant que ce n'est pas tranché, aucune application composée n'a de fenêtre.
- Le `MEIP` de QEMU dans le chemin USB-Serial-JTAG (`usb_serial_jtag_echo`) : cause non identifiée, à lire dans `hw/riscv/esp32c3_intmatrix.c` et le modèle USB-Serial-JTAG de QEMU.
- CrossPoint au-delà du capteur de température : QEMU n'a pas de TSENS ; aller plus loin demande un oracle qui l'a (le banc du sous-projet 8).
- **Sous-projet 8** : les registres que le reset de la puce garde ou remet (hypothèse de `Hart::reset` sur les registres entiers), le `CLKDIV` de l'UART après un reset du CPU, et chaque ligne (ii).

## Limites de preuve

- Vérifié en local : `cargo fmt --all --check` ; clippy et rustdoc (`-D warnings`) sur `lockstep` et `inkemu-lockstep` ; `cargo test -p lockstep -p inkemu-lockstep` ; la compilation WebAssembly de `lockstep` ; l'empreinte de 12 s ; contre QEMU dans Docker, les trois fenêtres de la CI, la faute après le reset, les cinq applications composées jusqu'à leur arrêt, et CrossPoint une fois.
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace`, les suites ignorées, la couverture, les étapes du lockstep dans la CI elle-même.
- **Jamais exécuté dans la CI** : la fenêtre `crosspoint` (le fichier est celui de l'utilisateur). Son résultat est celui du 2026-10-14 ; une autre build de QEMU ou une autre révision d'inkemu demandent de le refaire.
- La longueur de chaque fenêtre ne dépend pas de l'aléa de QEMU d'après les mesures répétées ; rien ne le garantit pour un autre firmware (ligne 17).
