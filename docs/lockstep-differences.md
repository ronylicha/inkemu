# Différences admises entre inkemu et QEMU (lockstep)

Le lockstep (`inkemu-lockstep`, crate `lockstep`, ADR 0008) compare après chaque instruction le PC,
`x1`–`x31` et les huit CSR machine que les deux côtés ont (`mstatus`, `mie`, `mip`, `mtvec`,
`mepc`, `mcause`, `mtval`, `mscratch`). Toute différence qui n'est pas une ligne de ce tableau est
une divergence (décision H3). Deux contrôles tiennent ce tableau :

- **Le tableau de l'outil est celui-ci.** `lockstep::DIFFERENCES` (`crates/lockstep/src/table.rs`)
  et ce document ont les mêmes lignes, les mêmes traitements et les mêmes paramètres, registre par
  registre et bit par bit : `crates/lockstep/tests/table.rs` compare chaque bloc « Paramètres »
  ci-dessous à la ligne de l'outil.
- **Chaque ligne sert, partout où elle déclare quelque chose, et seulement là.** L'outil
  enregistre ce que chaque ligne a fait pendant la fenêtre : les registres, les bits pris à QEMU, le
  mouvement de l'écart d'un compteur, ce qu'un réglage a changé.
  - L'exécution échoue (code de sortie 5, `lockstep::Usage::check`, que la bibliothèque offre par
    `Lockstep::check_usage` et que l'outil appelle) dans trois cas : une ligne utilisée hors de ses
    registres ou de ses bits, un compteur au-delà de sa borne, une partie de ligne jamais utilisée.
    Une partie, c'est une ligne entière, un de ses registres (une entrée morte dans une ligne qui
    sert ailleurs), ou un bit de son masque jamais pris à QEMU.
  - Une fenêtre coupée par `--max-instructions` liste ce qu'elle laisse inutilisé sans échouer.
  - L'outil imprime cet usage à chaque exécution, avec le plus grand mouvement de chaque compteur.
    La CI épingle le nombre d'usages de chaque ligne de chaque fenêtre (`--expect-usage
    tools/ci/lockstep-<fenêtre>.usage`, code 3 s'il change).
- **Chaque fenêtre a ses lignes** (SP1b-4d, décision J3). Une fenêtre compare un firmware ; elle dit
  combien de resets des deux côtés la comparaison traverse, sa borne éventuelle, et la liste des
  lignes qui valent pour elle. Une ligne a les mêmes paramètres dans toutes les fenêtres qui la
  prennent, et chacune de ces fenêtres l'utilise entièrement ; un firmware qui montre d'autres bits ou
  d'autres registres reçoit une ligne à lui, mesurée sur lui. `crates/lockstep/tests/table.rs`
  compare aussi la section « Les fenêtres » à `lockstep::WINDOWS`.

QEMU : `esp_develop_9.2.2_20250817`, celui de l'image `espressif/idf:v5.5.2`
(`tools/conformance/versions.env`). Les lignes 1 à 24 ont été mesurées le 2026-10-14 (SP1b-4c) sur la
première vie de `flash_hello.bin` (ROM, bootloader, application jusqu'à `esp_restart`, 3 402 169
instructions comparées), les lignes suivantes le même jour (SP1b-4d) sur les fenêtres qu'elles
nomment ; chaque masque est l'ensemble des bits mesurés différents, rien de plus. Un firmware qui
montre un autre bit, ou touche un autre registre, échoue jusqu'à ce qu'une ligne l'admette avec sa
preuve. Une ligne qui n'a plus d'objet est retirée sans que son numéro soit repris (ligne 1, retirée
le 2026-10-14) : un numéro désigne toujours la même différence, dans ce tableau comme dans l'ADR 0008.

## Les fenêtres (SP1b-4d)

Chaque fenêtre, avec le nombre d'instructions comparées, sa fin et la date de la mesure, que la CI
épingle (`--expect-compared`, `--expect-end`, `--expect-usage`). « Resets » est le nombre de resets
des deux côtés que la comparaison traverse ; le suivant termine la fenêtre.

| Fenêtre | Firmware | Resets | Borne | Lignes | Comparées | Fin |
|---|---|---|---|---|---|---|
| hello | `flash_hello.bin` | 1 | — | 2–28 | 6 813 949 | reset |
| nvs | `flash_nvs.bin` | 1 | — | 2–6, 8–24, 26, 27, 29–32 | 7 680 555 | reset |
| nvs-ota | `flash_nvs_ota.bin` | 1 | — | 2–6, 8–24, 26, 27, 29–33 | 7 701 890 | reset |
| crosspoint | `crosspoint-1.6.5-x3-x4.bin` | 0 | — | 2, 3, 5, 6, 8, 10, 12, 13, 16–19, 21, 22, 33–41 | 31 088 420 | registre absent de QEMU |

- **hello** : la ROM, le bootloader, `app_main` et le compte à rebours, le reset que demande
  `esp_restart`, puis une seconde vie entière, jusqu'au second reset. 2 224 interruptions prises par
  les deux côtés à la même instruction, 100 blocs réimprimés et 20 préemptés écartés.
- **nvs** : de même pour `nvs_rw_value_cxx`, qui écrit son compteur de redémarrages dans la NVS avant
  chaque `esp_restart` : les programmes et les effacements de la flash par SPI1, sous QEMU comme sous
  inkemu, sont dans la fenêtre. 2 224 interruptions, 114 blocs réimprimés, 20 préemptés.
- **nvs-ota** : la même application sous la table de partitions d'origine de la X4, dont le
  bootloader écrit `otadata` à chaque démarrage ; les lignes de `nvs` et la ligne 33. 2 224
  interruptions, 118 blocs réimprimés, 20 préemptés.
- **crosspoint** : CrossPoint 1.6.5 pour la X3 et la X4 (`crosspoint-1.6.5-x3-x4.bin`, SHA-256
  `8eccf614…0385764`) derrière le bootloader qu'inkemu livre, composés comme le fait
  `inkemu run --firmware`, sous la table de partitions de la X4. Ce fichier n'est pas dans le dépôt
  et ne tourne jamais dans la CI : `crates/lockstep-cli/tests/crosspoint.rs`, ignoré, lit le fichier
  de l'utilisateur (`INKEMU_CROSSPOINT_X3_X4`), refuse tout autre fichier, et épingle le compte, la
  fin et les usages (`crates/lockstep-cli/tests/crosspoint.usage`). Mesure locale du 2026-10-14 :
  la ROM, le bootloader (environ 30,7 millions d'instructions, dont l'écriture d'`otadata`) et le
  début de l'application ; la fenêtre se termine au premier accès à un registre que QEMU n'a pas et
  qu'aucune ligne ne nomme, la lecture d'`APB_SARADC.APB_TSENS_CTRL` (`0x60040058`, le capteur de
  température) au PC `0x4214917C`, avant le démarrage de l'ordonnanceur. Aucune interruption, 657
  blocs réimprimés, aucun préempté. Le compte n'est pas l'objectif d'environ 32 millions de la
  spécification de SP1b-4d : c'est la fin de la fenêtre, à cet accès, et non une borne (décision
  R41 du plan). Aucune ligne n'admet ce registre : QEMU n'a pas de capteur de température, et une
  copie donnerait à CrossPoint une valeur qu'aucun des deux côtés n'a mesurée.

Les autres firmwares de référence n'ont pas de fenêtre (SP1b-4d, décision J5) : chacun s'arrête,
mesuré le 2026-10-14, dans le démarrage de son application, à une différence d'instant
d'interruption dont la cause n'est pas établie, sans fin à lui avant. `adc_wait_panic` (3 986 033
instructions comparées), `uart_select` (3 963 178), `usb_serial_jtag_console` (3 848 562) et
`deep_sleep_timer` (3 921 093) : le premier tick de FreeRTOS tombe pendant que le CPU exécute, et
QEMU le prend deux instructions après inkemu (dans `hello` et `nvs`, chaque tick tombe dans un
`wfi`). inkemu ancre l'alarme au début du rangement qui lance le compteur, une hypothèse
(`soc_esp32c3::periph::systimer`, `write_conf`) qui explique l'une des deux instructions ; la puce,
elle, ne peut que retarder un tel rangement (bus APB, domaine d'horloge à 16 MHz). L'écart n'est
donc pas attribué à QEMU. `usb_serial_jtag_echo` (2 525 511) : après que le gestionnaire
d'interruptions a rouvert `MIE`, QEMU montre `MEIP` alors que sa matrice, calculée sur l'état du
contrôleur d'inkemu et des sources d'inkemu, ne lève pas la ligne ; les deux modèles de
l'USB-Serial-JTAG diffèrent (ligne 19), et une source haute chez QEMU et basse chez inkemu donnerait
la même observation. Ni l'un ni l'autre n'est admis : ce serait une ligne sur l'instant d'une
interruption, que le lockstep doit prouver. La note de passation de SP1b-4d donne la mesure.

## La fin d'une fenêtre (H4, corrigée le 2026-10-14, puis par SP1b-4d)

La comparaison traverse les resets des deux côtés à la fois que sa fenêtre déclare (SP1b-4d,
décisions J1 et J2, ci-dessous), et s'arrête au suivant. Elle s'arrête aussi, sans divergence, à la
borne de la fenêtre quand elle en a une, et au premier
accès (chargement ou rangement, de toute largeur) à une adresse de la fenêtre des périphériques
(`0x6000_0000`–`0x600F_FFFF`) qu'aucun périphérique de QEMU ne couvre, sauf quand une ligne nomme ce
registre. Un reset d'un seul côté est une divergence. Les périphériques de QEMU sont ceux de son
arbre mémoire aplati (`info mtree -f` de l'image épinglée, relevé le 2026-10-14), à l'octet près :

| Périphérique | Premier octet | Dernier octet |
|---|---|---|
| UART0 | `0x6000_0000` | `0x6000_007B` |
| SPI1 | `0x6000_2000` | `0x6000_23FF` |
| GPIO | `0x6000_4000` | `0x6000_4FFF` |
| RTC_CNTL | `0x6000_8000` | `0x6000_81FF` |
| EFUSE | `0x6000_8800` | `0x6000_8A03` |
| UART1 | `0x6001_0000` | `0x6001_007B` |
| TIMG0 | `0x6001_F000` | `0x6001_F0FF` |
| TIMG1 | `0x6002_0000` | `0x6002_00FF` |
| SYSTIMER | `0x6002_3000` | `0x6002_30FF` |
| TWAI | `0x6002_B000` | `0x6002_B1FF` |
| AES | `0x6003_A000` | `0x6003_A0BB` |
| SHA | `0x6003_B000` | `0x6003_B0FF` |
| RSA | `0x6003_C000` | `0x6003_C833` |
| DS | `0x6003_D000` | `0x6003_DE23` |
| HMAC | `0x6003_E000` | `0x6003_E0FE` |
| DMA | `0x6003_F000` | `0x6003_F283` |
| USB_DEVICE | `0x6004_3000` | `0x6004_3083` |
| SYSTEM | `0x600C_0000` | `0x600C_009B` |
| INTERRUPT_CORE0 | `0x600C_2000` | `0x600C_27FF` |
| EXTMEM | `0x600C_4000` | `0x600C_51FF` |
| XTS_AES | `0x600C_C000` | `0x600C_C05F` |
| open_eth | `0x600C_D000` | `0x600C_D053` |
| open_eth | `0x600C_D400` | `0x600C_D7FF` |

Le reste est la zone fourre-tout `esp32c3.iomem` de QEMU, qui lit 0 et ignore les écritures, sauf
deux registres : `RNG.DATA` (ligne 17) et `SYSCON_ORIGIN` (`0x600263F8`, qui lit « QEMU » et termine
la fenêtre comme les autres). Cela couvre les blocs du composant que QEMU n'a pas (I2C0, LEDC, RMT,
SPI2, I2S0, APB_SARADC…), ceux du composant qu'il remplace par des zéros (IO_MUX, SPI0, APB_CTRL,
SENSITIVE, ASSIST_DEBUG), les registres du composant absents du SVD (le maître I²C analogique, la
radio : FE, FE2, NRX, BB) et les registres au-delà de ce que QEMU modélise d'un bloc (RTC_CNTL au-delà
de `0x1FF`, SYSTEM au-delà de `0x9B`).

### Traverser un reset (SP1b-4d)

`esp_restart` finit par `esp_rom_software_reset_cpu(0)` (ESP-IDF v5.5.2,
`components/esp_system/port/soc/esp32c3/system_internal.c`, lu dans l'image épinglée) : le bit
`SW_PROCPU_RST` de `RTC_CNTL.OPTIONS0`, la cause `RTC_SW_CPU_RST` (0x0C), un reset du CPU, qui « ne
réinitialise que le cœur » (TRM §6.1, tableau 6.1-1). Avant lui, `esp_system_reset_modules_on_exit`
pulse `SYSTEM.PERIP_RST_EN0` (`0x20000007`) et `PERIP_RST_EN1` (`0x7E`). inkemu modélise le reset
du CPU (`ResetCause::SoftwareCpu`, niveau `ResetLevel::Cpu`) et, de ces impulsions, ce qu'il a de
modules : SPI0/1 (bit 1), UART0 (bit 2) et le SYSTIMER (bit 29) de `PERIP_RST_EN0`, SHA (bit 2) de
`PERIP_RST_EN1`. Le bit 0 de `PERIP_RST_EN0` (`SYSTEM_TIMERS_RST`) ne remet rien à zéro dans inkemu :
c'est une hypothèse (`soc_esp32c3::periph::system`, rapport IR §4.3 et §7 question 1), et les
groupes de timers TIMG0 et TIMG1 gardent leur état à travers le reset (voir la ligne 27). DMA, AES,
RSA, DS, HMAC et la radio ne sont pas des modules qu'inkemu remet à zéro. QEMU, lui, réinitialise
sa machine entière, mais pas tous ses registres (ligne 28). La référence est la puce : inkemu n'est
pas modifié.

Au reset des deux côtés, rien de la machine n'est touché. Ce que le lockstep savait de QEMU repart
comme à la mise sous tension : aucun CSR écrit (les paires « jusqu'à écriture » valent de nouveau),
l'écart des compteurs refixé par leur prochaine lecture (QEMU continue son compteur de cycles et le
compteur 0 de son SYSTIMER à travers le reset, rapport IR §4.3 ; inkemu repart de 0, une hypothèse
de `cpu_rv32::Hart::reset` pour le premier, l'impulsion du bit 29 pour le second ; l'écart est
refixé, rien n'est supposé de son origine), aucune commande SPI1, aucun retard de `MEIP`. La capture du
temporisateur RTC reste : celui de la puce compte à travers un reset du CPU, et QEMU n'en a pas. Tout
ce que les deux resets laissent différent est une ligne mesurée sur la fenêtre (lignes 25 à 28, 31 et
32), ou une divergence ; aucune resynchronisation.

## Traitements, par ordre de préférence (H3)

- **(i) paire de valeurs** : des bits d'un CSR comparé valent exactement une valeur dans QEMU et une
  autre dans inkemu ; les autres bits sont comparés, et une lecture du CSR dans un registre est tenue
  à la même paire. Une paire « jusqu'à écriture » ne vaut que jusqu'à ce qu'une instruction écrive le
  CSR. Le bit `MEIP` de `mip` (ligne 4) est tenu à la ligne que la matrice de QEMU calcule dans l'état
  du contrôleur d'inkemu. **(i) gardé au reset** (SP1b-4d) : un registre entier ou un CSR comparé que
  QEMU garde à son reset, là où le reset du CPU d'inkemu le remet à sa valeur de reset ; d'un CSR,
  seuls les bits mesurés sont tenus, les autres sont comparés. Au reset, s'ils diffèrent dans ces
  bits, la valeur de QEMU doit être celle qu'il avait avant le reset (l'état d'où partait
  l'instruction qui l'a demandé, qu'elle n'écrit pas), et celle d'inkemu la valeur de reset que la
  ligne déclare (`cpu_rv32::Hart::reset`) : un reset d'inkemu qui laisserait une autre valeur est une
  divergence. La paire tient exactement jusqu'à ce qu'une instruction l'écrive ou qu'un piège change
  l'une ou l'autre. Une instruction qui le lit pendant ce temps est une divergence (cause « read …,
  which the last reset left different »), une lecture-modification-écriture d'un CSR comprise, sauf
  un rangement d'un mot : il porte la paire en mémoire, comme un prologue de fonction sauve un
  registre qu'il rendra, et le chargement d'un mot qui la rend exactement des deux côtés tient sa
  destination à son tour ; un autre rangement sur ce mot l'efface.
- **(ii) copie de bits** : après un chargement d'un mot depuis un registre nommé, le registre
  destination d'inkemu prend les bits du masque tels que QEMU les a chargés, les autres bits devant
  être égaux. Un registre de masque 0 est un accès admis à un registre que QEMU n'a pas : rien n'est
  copié, la valeur est comparée. Une copie peut porter une condition (la commande SPI1 qui a précédé)
  et une valeur que QEMU doit avoir lue. **(ii) compteur tenu** : la valeur d'inkemu est celle de
  son propre modèle, et l'écart avec celle de QEMU (d'une autre origine) ne peut bouger que comme la
  ligne le dit et le source. Une lecture dans la borne prend la valeur de QEMU ; au-delà, c'est une
  divergence. **(ii) horloge lente** : un temporisateur que QEMU n'a pas doit valoir, dans inkemu,
  les cycles de sa fréquence déclarée au temps de l'appareil de sa capture. **(ii) copie après
  reset** (SP1b-4d) : la même copie de bits, faite seulement après un reset qui a laissé le registre
  dans deux états différents, et jusqu'au premier rangement dans ce registre depuis. Deux resets le
  font : celui des deux côtés que la fenêtre traverse (QEMU remet sa machine à ses valeurs, le reset
  du CPU garde les périphériques), et celui d'un module qu'inkemu fait à une impulsion de son bit de
  `SYSTEM.PERIP_RST_EN0/1`, que QEMU ignore (`hw/riscv/esp32c3_clk.c` : `PERIP_RST_EN0/1` y lisent 0
  et les écritures n'y font rien). Les modules sont ceux qu'inkemu remet à zéro (SPI0/1, UART0, UART1,
  TIMG0, TIMG1, USB-Serial-JTAG, SYSTIMER, SHA ; `lockstep::table::MODULE_RESETS`). La copie s'ajoute
  à ce que prennent les registres des lignes (ii) à la même adresse (un bit va à la première ligne qui
  le déclare). **(ii) CSR gardé au reset** (SP1b-4d) : un CSR que QEMU n'imprime pas, dont son reset
  garde certains bits ; après un reset, et jusqu'à ce qu'une instruction écrive le CSR en entier
  (`csrrw`, `csrrwi`), une lecture doit donner dans QEMU la valeur d'inkemu avec, en plus, ceux de
  ces bits qu'inkemu avait juste avant le reset, et la destination prend les bits de QEMU, dans le
  masque mesuré. Un `csrrs` ou un `csrrc` réécrit la valeur qu'il vient de lire, que la ligne a
  admise, avec des bits posés ou effacés : la différence qu'il porte reste celle du reset.
- **(iii) resynchronisation** à un PC d'ancrage : aucune ligne n'en a eu besoin.
- **(iv) réglage `QemuCompat`** de la machine, désactivé par défaut, que seul le lockstep allume,
  quand la différence change le flot de contrôle et qu'aucun des trois premiers ne suffit. La ligne
  compte une utilisation chaque fois que le réglage a changé quelque chose (la machine le compte).

## Tableau

La colonne « Usages » donne ce que chaque ligne a fait dans chaque fenêtre qui la prend : une
comparaison qu'une paire règle, une valeur copiée, un accès admis, une action d'un réglage. Les
usages de SP1b-4c (première vie de `flash_hello.bin` seule) sont dans l'ADR 0008.

| N° | Différence | Traitement | Source | Usages |
|---|---|---|---|---|
| 1 | `mstatus.MPP` : retirée le 2026-10-14, le `MPP` d'inkemu est celui de la puce | retirée | TRM registre 1.5, §1.4.2 (rapport IR §2.3) | — |
| 2 | `mie` au reset : QEMU `0x0EEE`, inkemu 0, jusqu'à ce que le firmware l'écrive | (i) paire | oracle §1c piège 6 ; `target/riscv/esp_cpu.c` | hello 6 813 950 ; nvs 7 680 556 ; nvs-ota 7 701 891 ; crosspoint 31 088 421 |
| 3 | `mtvec.MODE` au reset : QEMU direct, inkemu vectorisé, jusqu'à l'écriture de `mtvec` | (i) paire | `crates/cpu-rv32/src/csr.rs` (`MTVEC_VECTORED`) ; oracle §1c | hello 15 ; nvs 15 ; nvs-ota 15 ; crosspoint 15 |
| 4 | `mip.MEIP` : la ligne de la matrice de QEMU, trois instructions en retard après un rangement du seuil qui la fait tomber | (i) paire | `hw/riscv/esp32c3_intmatrix.c` ; `target/riscv/esp_cpu.c` ; mesure | hello 188 988 ; nvs 188 988 ; nvs-ota 188 988 |
| 5 | Le compteur de cycles : les deux comptent 6,25 ns du temps de l'appareil, celui de QEMU depuis sa première lecture | (iv) réglage ; (ii) compteur tenu | `target/riscv/esp_cpu.c` (`esp_cpu_get_cycles`) ; mesure | hello 1 118 166 ; nvs 1 118 170 ; nvs-ota 1 118 182 ; crosspoint 429 162 |
| 6 | UART0 : niveaux de ligne, compte de FIFO et valeurs de reset que QEMU ne modélise pas | (ii) copie | mesure ; `info mtree -f` (`esp_soc.uart`) | hello 6 483 ; nvs 6 601 ; nvs-ota 7 150 ; crosspoint 1 955 |
| 7 | SPI1 : la flash de QEMU ne répond pas à SFDP, laisse `W0` périmé et n'a pas les valeurs de reset | (ii) copie | oracle §1b (`Unknown cmd 5a`) ; mesure | hello 40 |
| 8 | SPI0, absent de QEMU : la configuration flash du cache | (ii) copie | `info mtree -f` ; mesure | hello 162 ; nvs 162 ; nvs-ota 169 ; crosspoint 88 |
| 9 | RTC_CNTL : QEMU ne modélise que `OPTIONS0`, `RESET_STATE` et `STORE0`–`7` | (ii) horloge lente ; (ii) copie | `hw/misc/esp32c3_rtc_cntl.c` ; `clk_tree_defs.h` ; mesure | hello 165 ; nvs 165 ; nvs-ota 165 |
| 10 | eFuse : les mots où les eFuses vierges de QEMU et ceux de la X4 diffèrent | (ii) copie | oracle §1d ; carryover SP1b-2 ligne 17 ; mesure | hello 132 ; nvs 126 ; nvs-ota 126 ; crosspoint 42 |
| 11 | IO_MUX, absent de QEMU : la configuration des broches GPIO0–21 | (ii) copie | `info mtree -f` ; SVD ; mesure | hello 549 ; nvs 549 ; nvs-ota 549 |
| 12 | Le maître I²C analogique, absent de QEMU : les accès regi2c de la ROM | (ii) copie | carryover SP1b-2 lignes 7 et 8 ; mesure | hello 268 ; nvs 268 ; nvs-ota 268 ; crosspoint 180 |
| 13 | La mise sous tension de la radio par `rtc_sleep_pu`, absente de QEMU : BB, NRX, FE, FE2 | (ii) copie | ESP-IDF v5.5.2 `rtc_sleep.c` ; `esp32c3-bootloader-accesses.md` lignes 31–33 | hello 28 ; nvs 28 ; nvs-ota 28 ; crosspoint 14 |
| 14 | TIMG0 et TIMG1 : valeurs de reset des chiens de garde, portes d'horloge, calibration RTC | (ii) copie | `timers-and-watchdogs` §G lignes 1, 5 et 8 ; mesure | hello 24 ; nvs 24 ; nvs-ota 24 |
| 15 | SYSTIMER : les compteurs de QEMU perdent une fraction d'unité à chaque mise à jour et se lisent toujours valides | (ii) copie ; (ii) compteur tenu | `hw/timer/esp_systimer.c` ; SVD ; mesure | hello 4 442 ; nvs 4 714 ; nvs-ota 4 678 |
| 16 | APB_CTRL, absent de QEMU : horloges Wi-Fi et alimentation des mémoires | (ii) copie | `info mtree -f` ; mesure | hello 43 ; nvs 43 ; nvs-ota 43 ; crosspoint 18 |
| 17 | `RNG.DATA` : QEMU rend le `rand()` de l'hôte | (ii) copie | `hw/riscv/esp32c3.c` (`A_SYSCON_RND_DATA_REG`) | hello 3 208 ; nvs 3 208 ; nvs-ota 3 208 ; crosspoint 1 604 |
| 18 | APB_SARADC, absent de QEMU : les sept registres de la source d'entropie du bootloader | (ii) copie | ESP-IDF v5.5.2 `bootloader_random_esp32c3.c` ; `esp32c3-bootloader-accesses.md` lignes 79–85 | hello 96 ; nvs 96 ; nvs-ota 96 ; crosspoint 48 |
| 19 | USB-Serial-JTAG : `SERIAL_IN_EP_DATA_FREE`, que QEMU lit 0 | (ii) copie | carryover SP1b-2 ligne 22 ; mesure | hello 14 218 ; nvs 14 231 ; nvs-ota 14 384 ; crosspoint 6 079 |
| 20 | SYSTEM : portes d'horloge, resets et choix d'horloge que le modèle d'horloge de QEMU tient autrement | (ii) copie | `hw/riscv/esp32c3_clk.c` ; `timers-and-watchdogs` §G ligne 12 ; mesure | hello 83 ; nvs 83 ; nvs-ota 83 |
| 21 | SENSITIVE, absent de QEMU : la protection mémoire qu'ESP-IDF installe | (ii) copie | `info mtree -f` ; mesure | hello 154 ; nvs 154 ; nvs-ota 154 ; crosspoint 77 |
| 22 | EXTMEM : bits d'alimentation, de gel et de synchronisation que le cache de QEMU ne tient pas | (ii) copie | carryover SP1b-2 ligne 3 ; mesure | hello 46 ; nvs 442 ; nvs-ota 388 ; crosspoint 24 |
| 23 | ASSIST_DEBUG, absent de QEMU : le moniteur de pile de FreeRTOS | (ii) copie | `timers-and-watchdogs` §G ligne 13 ; mesure | hello 28 892 ; nvs 28 892 ; nvs-ota 28 892 |
| 24 | SYSTIMER : pas d'alarme tant que le compteur est arrêté | (iv) réglage | `timers-and-watchdogs` §A.4 et §G ligne 10 ; `hw/timer/esp_systimer.c` | hello 2 ; nvs 2 ; nvs-ota 2 |
| 25 | `flash_hello.bin`, à son reset : QEMU garde des registres et des CSR que le reset du CPU d'inkemu remet à leur valeur de reset | (i) gardé au reset | `cpu_rv32::Hart::reset` (hypothèses pour les registres entiers et `mepc`) ; TRM §1.4.2 ; mesure | hello 4 789 611 |
| 26 | PMP : le reset de QEMU garde les bits R, W, X de chaque entrée, celui du CPU d'inkemu les efface | (ii) CSR gardé au reset | TRM §1.4.2 (rapport IR §4.2) ; spécification privilégiée RISC-V §3.7.1 ; mesure | hello 13 ; nvs 13 ; nvs-ota 13 |
| 27 | Après `esp_restart`, ou un reset de module que QEMU ignore : registres que les deux resets laissent dans des états différents, jusqu'au premier rangement | (ii) copie après reset | TRM §6.1 ; ESP-IDF v5.5.2 `system_internal.c` ; `hw/riscv/esp32c3_clk.c` ; mesure | hello 46 ; nvs 46 ; nvs-ota 46 |
| 28 | `flash_hello.bin`, après son reset : les registres de SPI1 que le reset de QEMU ne remet pas à leur valeur | (ii) copie après reset | `hw/ssi/esp32c3_spi.c` (`esp32c3_spi_reset_hold`) ; rapport IR §4.3 ; mesure | hello 4 |
| 29 | `flash_nvs.bin`, SPI1 : la ligne 7, et la flash de QEMU qui garde `WEL` après un programme ou un effacement | (ii) copie | `esp-idf-storage-flash-traffic.md` (résultat clé 5) ; mesure | nvs 600 ; nvs-ota 529 |
| 30 | `flash_nvs.bin`, EXTMEM : `ICACHE_SYNC_SIZE`, que le cache de QEMU lit toujours 0 | (ii) copie | `hw/misc/esp32c3_cache.c` (`esp32c3_cache_read`) ; mesure | nvs 2 ; nvs-ota 2 |
| 31 | `flash_nvs.bin`, à son reset : comme la ligne 25, avec d'autres registres | (i) gardé au reset | `cpu_rv32::Hart::reset` (hypothèses pour les registres entiers et `mepc`) ; TRM §1.4.2 ; mesure | nvs 5 108 772 ; nvs-ota 5 176 490 |
| 32 | `flash_nvs.bin`, après son reset : comme la ligne 28, avec d'autres bits | (ii) copie après reset | `hw/ssi/esp32c3_spi.c` (`esp32c3_spi_reset_hold`) ; rapport IR §4.3 ; mesure | nvs 4 ; nvs-ota 4 |
| 33 | `flash_nvs_ota.bin`, SPI1 : `RD_STATUS`, où la ROM lit le `WEL` que la flash de QEMU garde | (ii) copie | `esp-idf-storage-flash-traffic.md` (résultat clé 5) ; mesure | nvs-ota 10 ; crosspoint 9 |
| 34 | CrossPoint, SPI1 : la ligne 7, et le `WEL` que la flash de QEMU garde dans `W0` après `RDSR` | (ii) copie | `esp-idf-storage-flash-traffic.md` (résultat clé 5) ; mesure locale | crosspoint 21 |
| 35 | CrossPoint, eFuse : un mot du bloc 2 où les eFuses vierges de QEMU et ceux de la X4 diffèrent | (ii) copie | oracle §1d ; mesure locale | crosspoint 1 |
| 36 | CrossPoint, TIMG0 : les registres de la ligne 14 qu'il lit, et d'autres bits de la calibration RTC | (ii) copie | `timers-and-watchdogs` §G lignes 1 et 8 ; mesure locale | crosspoint 7 |
| 37 | CrossPoint, RTC_CNTL : la ligne 9, avec moins de bits de la configuration du chien de garde RTC | (ii) horloge lente ; (ii) copie | `hw/misc/esp32c3_rtc_cntl.c` ; `clk_tree_defs.h` ; mesure locale | crosspoint 86 |
| 38 | CrossPoint, IO_MUX : les broches de la flash (GPIO12–17) et de l'UART0 (GPIO20–21) | (ii) copie | `info mtree -f` ; SVD ; mesure locale | crosspoint 64 |
| 39 | CrossPoint, SYSTIMER : la ligne 15 pour le compteur 0, le seul lu avant l'ordonnanceur | (ii) copie ; (ii) compteur tenu | `hw/timer/esp_systimer.c` ; SVD ; mesure locale | crosspoint 13 |
| 40 | CrossPoint, SYSTEM : la ligne 20, un bit de moins dans `PERIP_RST_EN0` | (ii) copie | `timers-and-watchdogs` §G ligne 12 ; mesure locale | crosspoint 35 |
| 41 | CrossPoint, ASSIST_DEBUG : les trois registres que le bootloader et le démarrage touchent | (ii) copie | `timers-and-watchdogs` §G ligne 13 ; mesure locale | crosspoint 7 |

Les sources de QEMU sont celles de la branche `esp-develop` d'`espressif/qemu`. « Mesure » renvoie à
la mesure du 2026-10-14 : chaque registre de la fenêtre lu ou écrit, les valeurs des deux côtés à
chaque chargement, avec le PC (relevé décrit dans la note de passation de SP1b-4c).

## Paramètres et justification, ligne par ligne

Chaque bloc « Paramètres » est la ligne de l'outil, dans la syntaxe que le test lit : `paire <CSR>
bits <masque> qemu <valeur> inkemu <valeur> [jusqu'à écriture]`, `meip retard <instructions>`,
`registre <adresse> masque <bits> [après <commande SPI1>] [qemu <valeur>]`, `compteur <adresse ou
csr numéro> écart <unités> origines <…>` (l'écart tenu à ses unités de celui de l'origine) ou
`compteur <adresse> recul par mise à jour <premier>-<dernier> origines <…>` (l'écart ne descend que
d'une unité par mise à jour possible de QEMU entre deux lectures), `horloge lente <adresse> capture
<adresse> bit <n> <fréquence> Hz écart <cycles> qemu <valeur>`, `réglage <nom du champ de
QemuCompat>`, et depuis SP1b-4d `gardé au reset <registre ou CSR>`, `csr gardé au reset <adresse>
bits gardés <bits> masque <bits>`, `après reset registre <adresse> masque <bits>`. Une origine est un CSR écrit ou une adresse où un rangement est fait.

### Ligne 1 — `mstatus.MPP` (retirée le 2026-10-14)

La ligne admettait que QEMU lise `MPP` = 0 là où inkemu, cœur à mode machine seul, le lisait toujours
« machine » (`0b11`) : `mstatus` valait `0x1800` au reset et après chaque `mret`. Le TRM tranche
pour QEMU : `mstatus` vaut 0 au reset (§1.4.2) et `MPP` vaut 0 (utilisateur) ou 3 (machine), seul son
bit bas s'écrit (registre 1.5, cité au rapport IR §2.3). L'écart était donc un défaut d'inkemu, que
la ligne chargeait sur QEMU. Corrigé dans `cpu-rv32` (commit `3456f79`, avec ses tests) : sur le
cœur de l'ESP32-C3, `mstatus` repart de 0, une écriture prend `MPP` de son bit 11 et garde `TW`, un
piège met `MPP` à « machine », un `mret` à « utilisateur ». L'exécution en mode utilisateur reste
hors du modèle (SP1b-4f) : un `mret` vers le mode utilisateur continue en mode machine, ce qu'aucun
firmware mesuré ne fait. Les empreintes de trace épinglées n'ont pas changé (la suite des PC est la
même). Le numéro 1 n'est pas repris.

### Ligne 2 — `mie` au reset

La matrice de QEMU précharge `mie` à `0x0EEE` au reset (`target/riscv/esp_cpu.c`, commentaire de
`esp_cpu_has_work` : « the C3 intmatrix preloads MEIE in env->mie at reset ») ; inkemu part de 0, et
l'ESP32-C3 active ses lignes dans la matrice d'interruptions, pas dans `mie` (décision E3). Mesure :
la seule paire observée est (`0x0EEE`, 0), et aucune instruction n'écrit `mie` dans la fenêtre. Dès
qu'une instruction l'écrit, la paire cesse et `mie` est comparé en entier.

Paramètres :

```text
paire mie bits 0x00000EEE qemu 0x00000EEE inkemu 0x00000000 jusqu'à écriture
```

### Ligne 3 — `mtvec.MODE` au reset

inkemu câble le mode vectorisé dès le reset ; QEMU part de 0. La paire dure les 15 comparaisons qui
précèdent l'écriture de `mtvec` par la ROM.

Paramètres :

```text
paire mtvec bits 0x00000001 qemu 0x00000000 inkemu 0x00000001 jusqu'à écriture
```

### Ligne 4 — `mip.MEIP`

Le contrôleur d'inkemu ne montre pas les lignes au processeur par `mip`. Le cœur de QEMU met
`mip.MEIP` tant que sa matrice présente une ligne : activée, une source haute, une priorité au moins
égale au seuil, la priorité 0 comprise (`esp32c3_intmatrix_line_should_assert`,
`esp_cpu_update_parent_irq`). Le lockstep calcule cette condition sur l'état du contrôleur d'inkemu
(`Esp32C3::external_interrupt_as_qemu`) et exige que le `MEIP` de QEMU l'égale, celui d'inkemu restant
0 ; les autres bits de `mip` sont comparés. Mesure : égalité dans 91 158 comparaisons où QEMU montre
`MEIP` ; dans les 3 336 autres, exactement 3 × 1 112, QEMU garde `MEIP` les trois instructions qui
suivent le rangement du seuil `CPU_INT_THRESH` par `_interrupt_handler` (pc `0x40380252`), quand
le nouveau seuil a déjà fait tomber la ligne. Le mécanisme de ce retard dans QEMU n'est pas
identifié ; la ligne l'admet pour trois instructions après un tel rangement, pas une de plus, et
seulement après un rangement qui fait tomber la ligne : QEMU montrait `MEIP` à la frontière d'où
part le rangement, et sa matrice, calculée sur l'état du contrôleur d'inkemu, la donne basse après
lui. Un rangement du seuil qui ne fait rien tomber n'ouvre aucun retard. Une lecture de `mip` dans
un registre (`csrr`) est tenue de même : le registre d'inkemu prend le `MEIP` que l'état de QEMU
montrait à la frontière d'où part la lecture, déjà comparé ; tout autre bit lu est comparé.

Paramètres :

```text
meip retard 3
```

### Ligne 5 — le compteur de cycles

QEMU garde un compteur par adresse (`0x7E2`, `0x802`) qui compte des cycles de 6,25 ns de son horloge
virtuelle à partir de sa première lecture, reporte le reste de la division, et ignore les registres
d'événement et de mode ; une écriture fixe la valeur sans toucher l'instant de la dernière lecture,
et `csrw` ne lit pas (`esp_cpu_get_cycles`, `esp_cpu_csr_write`). Le CPU de QEMU ne change jamais
d'horloge.

La valeur qu'inkemu lit est celle de son propre compteur, celui de la puce : les registres
d'événement et de mode, une écriture qui fixe la valeur, un registre aux deux adresses. Le réglage
`cycle_counter_on_time_base` ne change que l'horloge qu'il compte : les cycles de la base de temps
(6,25 ns de temps de l'appareil) au lieu des cycles CPU que le modèle de temps facture. Sous
`qemu-icount` une instruction dure 8 ns quelle que soit l'horloge CPU, et la base de temps est
l'horloge que les deux comptent. Sans lui, le compteur avance d'un cycle par instruction et l'écart
croît sans borne.

Le lockstep compare alors la valeur d'inkemu à celle de QEMU. Les origines diffèrent : QEMU part de
sa première lecture, inkemu de la mise sous tension. La première lecture de chaque adresse fixe donc
l'écart (valeur de QEMU moins valeur d'inkemu), et chaque lecture suivante doit rester à un cycle au
plus de cet écart, dans un sens ou dans l'autre. Une écriture de l'une ou l'autre adresse fixe une
nouvelle origine.

Source de la borne : les deux comptent `floor(n × 8 / 6,25)` cycles pour n instructions de 8 ns,
QEMU en reportant son reste depuis sa première lecture, inkemu en cycles entiers de la base de temps
(`floor(n × 32 / 25)`) depuis la mise sous tension. Deux parties entières du même temps, comptées
depuis deux origines, diffèrent au plus d'un.

Mesure du 2026-10-14 :

- `0x7E2` : 1 649 lectures, écart maximal 1 ;
- `0x802` : 281 572 lectures, écart maximal 1.

Une valeur dans la borne est copiée dans le registre d'inkemu : le firmware continue avec la valeur
de QEMU. Les usages comptent ces 283 221 lectures, plus les 283 221 lectures que le réglage a
servies.

Paramètres :

```text
réglage cycle_counter_on_time_base
compteur csr 0x7E2 écart 1 origines csr 0x7E2 csr 0x802
compteur csr 0x802 écart 1 origines csr 0x7E2 csr 0x802
```

### Ligne 6 — UART0

`STATUS` : QEMU lit 0 dans les niveaux de ligne (`CTSN`, `RXD`, `DTRN`, `RTSN`, `TXD`) et dans l'octet
bas de `TXFIFO_CNT`, sa FIFO d'émission se vidant aussitôt ; celle d'inkemu se vide au débit de la
ligne, et ses lignes sont au repos (1). La ROM attend la fin d'émission en lisant `STATUS` : la copie
décide du moment où elle cesse d'attendre, la FIFO d'inkemu continue de se vider. `CONF0` et
`CLK_CONF` : QEMU part de 0 où inkemu a les valeurs de reset du SVD ; `FSM_STATUS` : l'état
d'émission.

Paramètres :

```text
registre 0x6000001C masque 0xE0FFC000
registre 0x60000020 masque 0x1000001C
registre 0x6000006C masque 0x00000010
registre 0x60000078 masque 0x16722500
```

### Ligne 7 — SPI1

La flash de QEMU (M25P80 générique) ne connaît pas la lecture SFDP (`0x5A`, « Unknown cmd 5a ») : après
cette commande, `W0` garde l'octet reçu d'inkemu (`0xFF`) remplacé par une donnée antérieure. Après
`RDSR` (`0x05`), l'octet d'état est égal des deux côtés, mais QEMU laisse les trois octets hauts de
`W0` tels qu'ils étaient, inkemu les remet à 0. Chaque copie de `W0` n'est faite qu'après sa commande,
identifiée au rangement de `CMD.USR` par l'opcode que portait alors `USER2` ; la mesure trouve deux
chargements de `W0` différents, un après chacune. `MISC.CS1_DIS`, `FLASH_WAITI_CTRL.WAITI_CMD`,
`FLASH_SUS_CTRL` et `CLOCK_GATE` : valeurs de reset que le SPI1 de QEMU ne tient pas.

Paramètres :

```text
registre 0x60002034 masque 0x00000002
registre 0x60002058 masque 0x001860FF après 0x5A
registre 0x60002058 masque 0x00186000 après 0x05
registre 0x60002098 masque 0x00000014
registre 0x6000209C masque 0x0A002000
registre 0x600020DC masque 0x00000001
```

### Ligne 8 — SPI0

SPI0 n'est pas un périphérique de QEMU : la ROM y configure l'accès du cache à la flash, et QEMU lit 0.
Les bits copiés sont ceux qu'inkemu lit autrement ; `0x24` et `0x28`, que la ROM lit et écrit et que
le SVD ne décrit pas, sont admis à valeur égale.

Paramètres :

```text
registre 0x60003008 masque 0x00AC2008
registre 0x60003010 masque 0x00000061
registre 0x60003014 masque 0x00000000
registre 0x60003018 masque 0x700000C0
registre 0x6000301C masque 0x5C000007
registre 0x60003020 masque 0x70000003
registre 0x60003024 masque 0x00000000
registre 0x60003028 masque 0x00000000
registre 0x60003034 masque 0x00000000
registre 0x6000303C masque 0x00000001
registre 0x60003054 masque 0x00000200
registre 0x600030DC masque 0x00000001
registre 0x600030E0 masque 0x00000000
```

### Ligne 9 — RTC_CNTL

Le RTC_CNTL de QEMU ne lit que `OPTIONS0` (la valeur écrite, partie de 0), `RESET_STATE` (la cause
seule) et `STORE0`–`7` ; tout le reste lit 0. `OPTIONS0` et `RESET_STATE` sont donc comparés, sauf les
bits mesurés différents : les bits de forçage d'alimentation du quartz et de la PLL de la valeur de
reset d'`OPTIONS0` (`0x1C00A000` dans inkemu), et `STAT_VECTOR_SEL_APPCPU`/`PROCPU` (bits 12–13) de
`RESET_STATE`, que QEMU ne met pas ; la cause du reset est comparée. Les autres registres : valeurs
de reset, minuteries d'alimentation, horloges et chien de garde RTC qu'inkemu tient et que QEMU lit 0.

**Le temporisateur RTC (`TIME_LOW0`)** : QEMU ne le modélise pas et lit 0. Ce registre est l'un des
deux choix laissés par H4 corrigée, et la ligne prend le second (b) :

- (a) Faire terminer la fenêtre à sa lecture. Écarté : ses cinq lectures tombent avant `app_main`,
  et la fenêtre n'atteindrait ni `app_main` ni le premier tic de FreeRTOS.
- (b) Retenu. QEMU doit lire 0. La valeur d'inkemu doit être les cycles de l'horloge lente déclarée
  (136 kHz, `SOC_CLK_RC_SLOW_FREQ_APPROX`) au temps de l'appareil du dernier rangement du bit 31 de
  `TIME_UPDATE`, qui capture le temporisateur, ou 0 avant toute capture. La tolérance est d'un
  cycle de l'horloge lente : le lockstep relève le temps de l'appareil à la frontière qui suit le
  rangement, une instruction (8 ns) après la capture, bien moins qu'un cycle de l'horloge lente
  (7,35 µs). La valeur dans la tolérance prend celle de QEMU.

Mesure du 2026-10-14 : cinq lectures, écart 0. Une copie du mot entier sous la condition « QEMU lit
0 » ne vérifiait rien : elle a été retirée.

Le bit 31 de `0x600080AC` (SP1b-4d) : la première vie le pose, le reset du CPU le garde (RTC_CNTL
n'est pas remis à zéro), et la seconde vie le relit, après l'avoir rangé, là où QEMU, qui ne modélise
pas ce registre, lit 0. Ce n'est pas une différence de reset, qu'un rangement effacerait : c'est la
différence de modèle de la ligne, mesurée dans la seconde vie des trois fenêtres.

Paramètres :

```text
horloge lente 0x60008010 capture 0x6000800C bit 31 136000 Hz écart 1 qemu 0x00000000
registre 0x60008000 masque 0x1C00A000
registre 0x6000801C masque 0x3C140543
registre 0x60008024 masque 0x0A080A09
registre 0x60008028 masque 0x12200A09
registre 0x6000802C masque 0x00008000
registre 0x60008030 masque 0x0A080000
registre 0x60008034 masque 0x00D40000
registre 0x60008038 masque 0x00003000
registre 0x60008040 masque 0x00000200
registre 0x60008070 masque 0x31D83298
registre 0x60008074 masque 0x00400000
registre 0x60008080 masque 0xA03FC000
registre 0x60008088 masque 0x00575010
registre 0x6000808C masque 0xBA805480
registre 0x60008090 masque 0xF807FA14
registre 0x600080AC masque 0x84B00000
registre 0x600080D8 masque 0x7BFFC030
registre 0x6000810C masque 0x00000007
registre 0x6000811C masque 0xC0000000
```

### Ligne 10 — eFuse

QEMU démarre sur des eFuses vierges (puce de révision 1.3, sans MAC ni calibration) ; ceux d'inkemu
sont ceux du profil X4 (révision 0.3 de production, bloc v1.2). Les contrôleurs sont comparés : seuls
les cinq mots de données lus différents sont copiés, et seulement leurs bits mesurés différents.

Paramètres :

```text
registre 0x6000883C masque 0x80000000
registre 0x60008848 masque 0x000058CF
registre 0x60008850 masque 0x01000000
registre 0x60008858 masque 0x0047D2F2
registre 0x6000886C masque 0xD864EA40
```

### Ligne 11 — IO_MUX

IO_MUX n'est pas un périphérique de QEMU. inkemu a la valeur de reset du SVD (`FUN_IE`, `FUN_DRV` = 2 :
`0xB00`) et ce que la ROM et ESP-IDF y écrivent (le bit 12 de `MCU_SEL` pour GPIO12–13, les tirages
de GPIO14–17 pour la flash) ; QEMU lit 0.

Paramètres :

```text
registre 0x60009004 masque 0x00000B00
registre 0x60009008 masque 0x00000B00
registre 0x6000900C masque 0x00000B00
registre 0x60009010 masque 0x00000B00
registre 0x60009014 masque 0x00000B00
registre 0x60009018 masque 0x00000B00
registre 0x6000901C masque 0x00000B00
registre 0x60009020 masque 0x00000B00
registre 0x60009024 masque 0x00000B00
registre 0x60009028 masque 0x00000B00
registre 0x6000902C masque 0x00000B00
registre 0x60009030 masque 0x00000B00
registre 0x60009034 masque 0x00001F00
registre 0x60009038 masque 0x00001F00
registre 0x6000903C masque 0x00000F00
registre 0x60009040 masque 0x00000F00
registre 0x60009044 masque 0x00000F00
registre 0x60009048 masque 0x00000F00
registre 0x6000904C masque 0x00000B00
registre 0x60009050 masque 0x00000B00
registre 0x60009054 masque 0x00000B00
registre 0x60009058 masque 0x00000B00
```

### Ligne 12 — le maître I²C analogique

Les routines regi2c de la ROM lisent ce bloc, absent du SVD et de QEMU, qui y lit `0xFFFFFF` dans les
deux registres de données ; inkemu modélise le maître et ses registres analogiques.

Paramètres :

```text
registre 0x6000E000 masque 0x05FFFF9E
registre 0x6000E044 masque 0x00FFFFFF
registre 0x6000E048 masque 0x00FFFFFF
```

### Ligne 13 — la mise sous tension de la radio

`rtc_sleep_pu` fait au démarrage de l'application une lecture-modification-écriture sur
`BB.BBPD_CTRL`, `NRX.NRXPD_CTRL` (`0x6001CCD4`, trois fois), `FE.FE_GEN_CTRL` (`0x60006090`) et
`FE2.FE2_TX_INTERP_CTRL` (`0x600050F0`), sans rien en attendre. Les deux côtés y lisent 0 : les
accès sont admis, rien n'est copié. Un firmware qui utilise le Wi-Fi touchera d'autres registres de
ces blocs et terminera la fenêtre.

Paramètres :

```text
registre 0x600050F0 masque 0x00000000
registre 0x60006090 masque 0x00000000
registre 0x6001CCD4 masque 0x00000000
registre 0x6001D054 masque 0x00000000
```

### Ligne 14 — TIMG0 et TIMG1

Le modèle `timer.esp.timg` de QEMU met à 0 la configuration des chiens de garde au reset (inkemu a la
valeur du SVD : `FLASHBOOT_MOD_EN`, longueurs de reset, prédiviseur), calcule la calibration de
l'horloge RTC autrement (`RTC_CALI_RDY`, la valeur, le dépassement) et lit 0 dans `REGCLK`.

Paramètres :

```text
registre 0x6001F048 masque 0x0004C000
registre 0x6001F04C masque 0x00010000
registre 0x6001F068 masque 0x00008000
registre 0x6001F06C masque 0x024C3C00
registre 0x6001F080 masque 0xFFFFFF99
registre 0x6001F0FC masque 0x60000000
registre 0x60020048 masque 0x0004C000
registre 0x6002004C masque 0x00010000
registre 0x600200FC masque 0x60000000
```

### Ligne 15 — les compteurs du SYSTIMER

La valeur qu'inkemu lit est celle de son propre SYSTIMER : ses compteurs à 16 MHz, sa capture
`UPDATE`, son `VALUE_VALID`, le reset du bloc par `SYSTEM.PERIP_RST_EN0`. Aucun réglage ne la
remplace.

Le SYSTIMER de QEMU (`hw/timer/esp_systimer.c`) diffère de trois façons :

- **Il tronque.** À chaque mise à jour d'un compteur (lecture `UPDATE`, chargement, alarme,
  reprogrammation d'un comparateur), il ajoute les unités entières écoulées depuis la précédente et
  perd le reste.
- **Il ne repart pas de zéro.** `SYSTEM.PERIP_RST_EN0` ne le remet jamais à zéro, et il met à jour un
  compteur arrêté quand il reprogramme un comparateur qui le suit.
- **`VALUE_VALID` se lit toujours 1** (« always synchronized and valid »). Le registre `UNITn_OP`
  d'inkemu lit sa valeur de reset du SVD, 0, jusqu'au premier `UPDATE`. ESP-IDF lit ce mot avant de
  poser `UPDATE` (lecture-modification-écriture de `systimer_ll_counter_snapshot`) : le bit 29 de
  `UNIT0_OP` et de `UNIT1_OP` est copié.

Les origines des deux compteurs diffèrent donc d'environ 300 000 unités (19 ms). La ligne ne les
compare pas : elle compare comment l'écart évolue. La première lecture d'un compteur après une
origine fixe l'écart (valeur de QEMU moins valeur d'inkemu). Sont des origines la mise sous tension,
un rangement dans `SYSTEM.PERIP_RST_EN0` (le reset du bloc qu'inkemu fait et que QEMU ignore) et un
rangement dans `SYSTIMER.CONF` (QEMU arrête et lance un compteur sans le mettre à jour). D'une
lecture à la suivante, l'écart ne peut que descendre, d'au plus une unité par mise à jour que QEMU a
pu faire entre elles : un rangement dans le bloc SYSTIMER (vidage, chargement, reprogrammation) ou
une interruption prise (une alarme).

Source de la borne :

- QEMU ajoute `floor(Δns × 16 / 1000)` à chaque mise à jour, inkemu compte `floor(cycles / 10)` de la
  base de temps.
- Les deux voient le même temps à la même instruction : n × 8 ns pour QEMU, `floor(n × 32 / 25)`
  cycles de 6,25 ns pour inkemu, soit le même nombre d'unités entières.
- Entre deux captures, l'avance d'inkemu est donc `floor(Δ)` ou `floor(Δ) + 1`. Celle de QEMU, une
  somme de k parties entières, est entre `floor(Δ) − (k − 1)` et `floor(Δ)`.
- L'écart bouge donc de 0 à −k. Il ne monte jamais.

Mesure du 2026-10-14 :

- compteur 0 : 18 lectures, mouvement maximal 1 ;
- compteur 1 : 2 200 lectures, mouvement maximal 1.

Limite de ce contrôle : un défaut d'inkemu qui décale le compteur d'une constante passe. Un défaut de
cadence n'est vu que s'il dépasse, entre deux lectures, la marge que les troncatures de QEMU lui
laissent.

Une valeur dans la borne est copiée dans le registre d'inkemu. Les usages comptent les 2 218 lectures
de compteur et les 2 copies de `VALUE_VALID`.

Paramètres :

```text
registre 0x60023004 masque 0x20000000
registre 0x60023008 masque 0x20000000
compteur 0x60023044 recul par mise à jour 0x60023000-0x600230FF origines 0x600C0018 0x60023000
compteur 0x6002304C recul par mise à jour 0x60023000-0x600230FF origines 0x600C0018 0x60023000
```

### Ligne 16 — APB_CTRL

APB_CTRL n'est pas un périphérique de QEMU : `WIFI_CLK_EN` (lu 0 où inkemu a sa valeur de reset puis
ce qu'ESP-IDF écrit), l'alimentation des mémoires du frontal radio et celle de la ROM et de la SRAM.
`WIFI_RST_EN` est admis à valeur égale.

Paramètres :

```text
registre 0x60026014 masque 0xFFFFFFFF
registre 0x60026018 masque 0x00000000
registre 0x6002609C masque 0x00000015
registre 0x600260AC masque 0x0000003F
```

### Ligne 17 — `RNG.DATA`

QEMU rend `rand()` de l'hôte, semé par `srand(time(NULL))` (`hw/riscv/esp32c3.c`) : la graine est la
seconde de l'horloge de l'hôte au lancement, et aucune option de QEMU ne la fixe. Ce n'est pas un
compteur ; le mot entier est copié, et inkemu continue avec la valeur de QEMU.

Ce qui rend la fenêtre indépendante de cette valeur :

- **La comparaison, par construction.** Inkemu prend à chaque lecture la valeur que QEMU a lue, et
  les deux côtés continuent avec le même mot. Ce qu'un firmware en fait ensuite (le ranger, le
  relire, brancher dessus) est identique des deux côtés.
- **La longueur de la fenêtre, par la mesure.** Seule cette longueur pourrait varier avec la
  graine : un branchement sur la valeur ferait suivre aux deux côtés, ensemble, un chemin plus long
  ou plus court. Trois captures de QEMU lancées à des secondes différentes le 2026-10-14 (deux
  exécutions vivantes et une trace enregistrée) comparent toutes 3 402 169 instructions, avec le
  même usage de chaque ligne. La CI épingle les deux.

Si un firmware de référence futur faisait varier sa longueur avec la graine, l'épinglage échouerait
(code 3) : il faudrait alors un firmware qui n'en dépend pas, pas une borne plus large.

Paramètres :

```text
registre 0x600260B0 masque 0xFFFFFFFF
```

### Ligne 18 — APB_SARADC

La source d'entropie du bootloader (`bootloader_random_enable`/`disable`) programme sept registres
d'APB_SARADC puis l'arrête, sans lire de mesure. Seuls ces sept registres sont admis ; une lecture de
mesure (`SAR1DATA_STATUS`, `INT_RAW`, `ONETIME_SAMPLE`…), celle d'une batterie ou de boutons sur une
échelle de résistances, termine la fenêtre.

Paramètres :

```text
registre 0x60040000 masque 0x588382C0
registre 0x60040004 masque 0x0106E1FE
registre 0x6004000C masque 0x00FF080D
registre 0x60040018 masque 0x009CFFFF
registre 0x6004001C masque 0x00FFFFFF
registre 0x60040050 masque 0x800000FF
registre 0x60040054 masque 0x0050000F
```

### Ligne 19 — `USB_DEVICE.EP1_CONF`

QEMU lit 0 dans `SERIAL_IN_EP_DATA_FREE` (aucun hôte ne vide son point d'accès) ; inkemu montre une
FIFO libre. La ROM attend une place puis renonce : la copie lui fait suivre le chemin de QEMU. Seul ce
bit est copié : `SERIAL_OUT_EP_DATA_AVAIL`, l'entrée de la console, est comparé.

Paramètres :

```text
registre 0x60043004 masque 0x00000002
```

### Ligne 20 — SYSTEM

Le modèle `esp32c3.soc.clk` de QEMU (`hw/riscv/esp32c3_clk.c`) ne lit que `CPU_PER_CONF`,
`SYSCLK_CONF`, `CPU_INTR_FROM_CPU_0`–`3` et `EXTERNAL_DEVICE_ENCRYPT_DECRYPT_CONTROL` : tout autre
registre, dont les portes d'horloge et `PERIP_RST_EN0/1`, lit 0 et ignore les écritures. `SYSCLK_CONF`
y lit la PLL sélectionnée. Dans `PERIP_RST_EN0` et `PERIP_RST_EN1`, les bits différents sont donc
tous ceux que le firmware pose : la seconde vie en pose d'autres que la première
(`esp_perip_clk_init`, après un reset du CPU, range `0xFF7FFFFF` et `0xFFFFFFC1`), d'où des masques
plus larges que ceux de SP1b-4c, mesurés sur les deux vies. La copie laisse inkemu dans un état que
le firmware ne voit pas : il croit la PLL déjà sélectionnée et saute sa mise en route, inkemu restant
sur le quartz. Sous `qemu-icount` le temps d'une instruction ne dépend pas de l'horloge ; les débits
dérivés de l'APB peuvent en dépendre, et ce chemin de changement d'horloge d'inkemu n'est pas exercé
par la fenêtre.

Paramètres :

```text
registre 0x600C0000 masque 0x00000040
registre 0x600C0004 masque 0x000000C0
registre 0x600C0008 masque 0x0000000C
registre 0x600C000C masque 0x00000001
registre 0x600C0010 masque 0xF9C1E06F
registre 0x600C0014 masque 0x00000204
registre 0x600C0018 masque 0xFF7FFFFF
registre 0x600C001C masque 0xFFFFFFFF
registre 0x600C0020 masque 0x000000FF
registre 0x600C0024 masque 0x02001001
registre 0x600C0040 masque 0x00000007
registre 0x600C0058 masque 0x00080401
```

### Ligne 21 — SENSITIVE

SENSITIVE n'est pas un périphérique de QEMU : ESP-IDF y installe la protection mémoire
(`esp_cpu_configure_region_protection` et les PMS). Les registres que la fenêtre lit avec une valeur
différente sont copiés sur leurs bits mesurés, les autres sont admis à valeur égale.

Paramètres :

```text
registre 0x600C101C masque 0x0000000F
registre 0x600C1090 masque 0x00000000
registre 0x600C1094 masque 0x00000000
registre 0x600C1098 masque 0x00000000
registre 0x600C109C masque 0x00000000
registre 0x600C10A0 masque 0x00000000
registre 0x600C10A4 masque 0x00000000
registre 0x600C10A8 masque 0x00000000
registre 0x600C10B0 masque 0x001C7FFF
registre 0x600C10B4 masque 0x00000000
registre 0x600C10B8 masque 0x00000003
registre 0x600C10C0 masque 0x00000000
registre 0x600C10C4 masque 0x0F0FF0FF
registre 0x600C10C8 masque 0x00000000
registre 0x600C10CC masque 0x00000003
registre 0x600C10D8 masque 0x00000000
registre 0x600C10FC masque 0x003FFFFF
registre 0x600C1100 masque 0x00000FFF
registre 0x600C1130 masque 0x00000000
registre 0x600C1134 masque 0x00000003
```

### Ligne 22 — EXTMEM

Le cache de QEMU ne tient pas les bits d'alimentation des étiquettes et de la MMU, de gel, de
synchronisation achevée et de propriétaire de la MMU comme le composant.

Paramètres :

```text
registre 0x600C4004 masque 0x00000003
registre 0x600C4008 masque 0x00000005
registre 0x600C4028 masque 0x00000002
registre 0x600C40AC masque 0x00000005
registre 0x600C40C4 masque 0x0000000F
registre 0x600C40CC masque 0x00000001
```

### Ligne 23 — ASSIST_DEBUG

ASSIST_DEBUG n'est pas un périphérique de QEMU : le moniteur de pile de FreeRTOS lit et réécrit
`CORE_0_MONTR_ENA` à chaque entrée d'interruption et range les bornes de pile. La copie des bits 8–9
laisse le moniteur d'inkemu désactivé après la première interruption, comme celui de QEMU : il n'est
pas exercé par la fenêtre.

Paramètres :

```text
registre 0x600CE000 masque 0x00000300
registre 0x600CE008 masque 0x00000000
registre 0x600CE038 masque 0x00000000
registre 0x600CE03C masque 0x00000000
registre 0x600CE044 masque 0x00000000
registre 0x600CE048 masque 0x00000000
registre 0x600CE098 masque 0x00000000
```

### Ligne 24 — pas d'alarme sur un compteur arrêté

Le SYSTIMER de QEMU ne programme pas d'alarme pour un comparateur dont le compteur est arrêté
(`esp_systimer_comparator_reprogram`) : le comparateur que `vSystimerSetup` active sur le compteur 1
encore arrêté ne donne pas l'interruption de tick précoce qu'inkemu donne. Le réglage reproduit ce
comportement pour tout comparateur, et compte chaque alarme retenue : une dans la fenêtre. Le chemin
du composant, cette interruption précoce, n'est donc pas comparé.

Paramètres :

```text
réglage systimer_no_alarm_on_stopped_counter
```

### Ligne 25 — `flash_hello.bin` : ce que QEMU garde à son reset

Au reset que demande `esp_restart`, le reset du CPU d'inkemu remet les registres entiers à 0, `mtvec`
à 1, `mepc` à 0 et `mstatus` à 0 (`cpu_rv32::Hart::reset`) ; chaque grandeur tenue l'est seulement
si inkemu montre cette valeur, que la ligne déclare. Deux de ces valeurs sont des hypothèses :
les registres entiers (marqués ASSUMPTION dans `Hart::reset` : l'architecture les laisse indéfinis,
et la ROM écrit chaque registre qu'elle lit) et `mepc` (« not established; ASSUMED reset with the
core », rapport IR §4.2) ; seul `mstatus` = 0 vient du TRM (§1.4.2). Un appareil les tranchera (sous-
projet 8). QEMU, qui réinitialise sa machine, garde les registres entiers, `mtvec`, `mepc` et le bit
`MPIE` de `mstatus`. Mesure : au reset, QEMU montre exactement les valeurs d'avant le reset, et ces
dix-huit grandeurs diffèrent ; de `mstatus`, seul `MPIE` (les autres bits sont comparés) ; aucune
autre, sauf `mie`, que la ligne 2 tient comme à la mise sous tension. Chacune est tenue jusqu'à ce
qu'une instruction l'écrive ou qu'un piège la change : la plupart dans les 6 000 premières
instructions de la ROM, `mepc` au premier tick de la seconde vie, 2,38 millions d'instructions plus
loin, et `tp` juste après. Aucune instruction ne les lit entre-temps, sauf des rangements d'un mot
sur la pile et des chargements qui les rendent : le mot `0x3FCDE6F4`, où la valeur de `s1` d'avant
le reset a été rangée, rechargé avec la même paire. La règle la plus stricte (toute autre lecture est
une divergence) passe donc : rien ne calcule avec une valeur que le reset a laissée.

Paramètres :

```text
gardé au reset x1/ra reset 0x00000000
gardé au reset x2/sp reset 0x00000000
gardé au reset x3/gp reset 0x00000000
gardé au reset x4/tp reset 0x00000000
gardé au reset x5/t0 reset 0x00000000
gardé au reset x8/s0 reset 0x00000000
gardé au reset x9/s1 reset 0x00000000
gardé au reset x11/a1 reset 0x00000000
gardé au reset x12/a2 reset 0x00000000
gardé au reset x13/a3 reset 0x00000000
gardé au reset x14/a4 reset 0x00000000
gardé au reset x15/a5 reset 0x00000000
gardé au reset x16/a6 reset 0x00000000
gardé au reset x17/a7 reset 0x00000000
gardé au reset x28/t3 reset 0x00000000
gardé au reset mstatus bits 0x00000080 reset 0x00000000
gardé au reset mtvec reset 0x00000001
gardé au reset mepc reset 0x00000000
```

### Ligne 26 — la configuration PMP

Le bootloader de la seconde vie lit `pmpcfg0` à `pmpcfg3` (`csrrs`) avant de les écrire. inkemu, dont
le reset du CPU remet les registres PMP à zéro (`cpu_rv32::Hart::reset`, comme la puce selon le TRM
§1.4.2, cité au rapport IR §4.2 : les CSR reprennent leur valeur de reset, et celle des registres
PMP est 0 ; que le reset efface aussi les bits L est une hypothèse du même rapport), lit 0 ; QEMU lit les bits R, W, X que la première vie avait mis, sans les bits L et A :
la spécification privilégiée RISC-V (§3.7.1) n'exige d'un reset que d'effacer A et L, et QEMU garde
le reste. Mesure : à chaque lecture, la valeur de QEMU est celle d'inkemu plus les bits R, W, X
qu'inkemu avait juste avant le reset (`0x89888F88` avant le reset pour `pmpcfg0`, `0x01000700` lu
par QEMU). La ligne tient jusqu'à ce qu'une instruction écrive le CSR en entier (`csrrw`) ; un
`csrrs` porte la différence qu'il a lue. inkemu ne fait pas respecter la PMP (SP1b-4f) : la copie ne
change que le registre destination. Mêmes masques dans `hello` et `nvs`.

Paramètres :

```text
csr gardé au reset 0x3A0 bits gardés 0x07070707 masque 0x01000700
csr gardé au reset 0x3A1 bits gardés 0x07070707 masque 0x00050103
csr gardé au reset 0x3A2 bits gardés 0x07070707 masque 0x07000507
csr gardé au reset 0x3A3 bits gardés 0x07070707 masque 0x00000300
```

### Ligne 27 — après `esp_restart`, ou un reset de module que QEMU ignore

Après le reset, la ROM, le bootloader et l'application relisent des registres que la première vie a
écrits. Le reset du CPU de la puce les garde (TRM §6.1), sauf les modules qu'`esp_restart` pulse
par `SYSTEM.PERIP_RST_EN0/1` ; QEMU remet sa machine à ses valeurs de reset. Les deux sens se
montrent :

- QEMU lit sa valeur de reset là où inkemu lit ce que la première vie a laissé : les `IN_SEL` de
  GPIO0–1 (`0x30`, rangés par `esp_restart`) ; le bit `SLP_SEL` (bit 1) des broches dans IO_MUX ;
  RTC_CNTL ; SYSTEM ; EXTMEM ; le PC qu'`ASSIST_DEBUG` a enregistré au reset, `0x40048B92`, que la
  ROM imprime en `Saved PC` ; TIMG0 et TIMG1. Pour ces deux derniers, c'est une hypothèse d'inkemu :
  `esp_restart` pulse le bit 0 de `PERIP_RST_EN0` (`SYSTEM_TIMERS_RST`), qu'inkemu suppose sans
  effet (`soc_esp32c3::periph::system`, rapport IR §4.3 et §7 question 1 : ce que ce bit remet à zéro
  n'est pas documenté). Si la puce remettait les groupes de timers à zéro à cette impulsion, ces
  quatre registres (`0x6001F048`, `0x6001F04C`, `0x6001F068`, `0x6002004C`) chargeraient QEMU d'un
  défaut d'inkemu. Un appareil le tranchera (sous-projet 8).
- QEMU garde l'ancienne valeur là où inkemu remet un module à zéro à une impulsion de son bit de
  `PERIP_RST_EN0/1`, que QEMU ignore (`hw/riscv/esp32c3_clk.c`) : UART0 (`CLKDIV` : QEMU `0x30015B`,
  le diviseur que la ROM avait programmé, inkemu `0x2B6`, sa valeur de reset), SPI1. Ces impulsions
  viennent d'`esp_restart` avant le reset, et de la seconde vie elle-même : la ROM pulse SPI0/1,
  `esp_perip_clk_init` range `0xFF7FFFFF` dans `PERIP_RST_EN0`.

Chaque registre est nommé avec les bits mesurés différents dans la seconde vie, au-delà de ce que
les lignes 2 à 24 prennent déjà à la même adresse. La copie ne se fait qu'après un reset qui a laissé
le registre dans deux états (celui des deux côtés que la fenêtre traverse, ou celui de son module par
inkemu seul), et jusqu'au premier rangement dans ce registre depuis : la valeur rangée est la même
des deux côtés, et toute différence qui suit est comparée. Dans la première vie, avant toute
impulsion d'un module, ces bits restent comparés. Mesure (relevé de chaque chargement et rangement
de ces registres dans la seconde vie de `hello`) : une lecture qui diffère après un rangement suit
toujours une impulsion du module de son registre (UART0 : `CLKDIV`, `0x60000078` ; SPI0/1 : `CTRL2`,
`USER2`, `MISO_DLEN`), sauf aux deux adresses que la ligne a quittées, où QEMU ne stocke rien : le
bit 31 de `0x600080AC` (RTC_CNTL, désormais à la ligne 9) et `PERIP_RST_EN0/1` (désormais à la
ligne 20). Mêmes registres et mêmes masques dans `hello`, `nvs` et `nvs-ota`.

Paramètres :

```text
après reset registre 0x60000014 masque 0x003003ED
après reset registre 0x60000078 masque 0x00001000
après reset registre 0x60002010 masque 0x000003E0
après reset registre 0x60002028 masque 0x00000017
après reset registre 0x60004154 masque 0x00000030
après reset registre 0x60004158 masque 0x00000030
après reset registre 0x60008024 masque 0x00010000
après reset registre 0x60008028 masque 0x00010000
après reset registre 0x6000802C masque 0x00000200
après reset registre 0x60008030 masque 0x00010000
après reset registre 0x60008080 masque 0x10000000
après reset registre 0x60008088 masque 0x00000800
après reset registre 0x6000808C masque 0x00400000
après reset registre 0x60009004 masque 0x00000002
après reset registre 0x60009008 masque 0x00000002
après reset registre 0x6000900C masque 0x00000002
après reset registre 0x60009010 masque 0x00000002
après reset registre 0x60009014 masque 0x00000002
après reset registre 0x60009018 masque 0x00000002
après reset registre 0x6000901C masque 0x00000002
après reset registre 0x60009020 masque 0x00000002
après reset registre 0x60009024 masque 0x00000002
après reset registre 0x60009028 masque 0x00000002
après reset registre 0x6000902C masque 0x00000002
après reset registre 0x60009030 masque 0x00000002
après reset registre 0x60009034 masque 0x00000002
après reset registre 0x60009038 masque 0x00000002
après reset registre 0x60009040 masque 0x00000002
après reset registre 0x60009044 masque 0x00000002
après reset registre 0x60009048 masque 0x00000002
après reset registre 0x6000904C masque 0x00000002
après reset registre 0x60009050 masque 0x00000002
après reset registre 0x6001F048 masque 0x383B0000
après reset registre 0x6001F04C masque 0x4E200000
après reset registre 0x6001F068 masque 0x04013000
après reset registre 0x6002004C masque 0x4E200000
après reset registre 0x60040018 masque 0x00630000
après reset registre 0x600C0008 masque 0x00000001
après reset registre 0x600C0024 masque 0x01000000
après reset registre 0x600C4078 masque 0x00000023
après reset registre 0x600C4084 masque 0x0000003F
après reset registre 0x600CE008 masque 0x00000300
après reset registre 0x600CE048 masque 0x40048B92
```

### Ligne 28 — `flash_hello.bin` : SPI1 après le reset

Deux registres de SPI1, `USER` (`0x60002018`) et `USER2` (`0x60002020`). inkemu les trouve à leurs
valeurs de reset après le reset (relevé de la seconde vie : `USER` `0x80000000`, `USER2`
`0x70000000`, `CTRL2` et `MISO_DLEN` 0, les valeurs du SVD), comme la puce, dont `esp_restart` pulse
SPI0/1. Le reset de QEMU, lui, ne remet que des champs : `esp32c3_spi_reset_hold`
(`hw/ssi/esp32c3_spi.c`) pose `USR_COMMAND` dans `USER` et `USR_COMMAND_BITLEN` dans `USER2` par
`FIELD_DP32` sur la valeur courante, sans effacer le reste, et ne touche ni `CTRL2` ni `MISO_DLEN`
(rapport IR §4.3 : « QEMU does not reset its SPI1 registers »). QEMU relit donc les bits que la
première vie avait rangés : la commande et les phases de la dernière opération sur la flash. Ils ne
sont pas les mêmes dans `hello` et dans `nvs` (ligne 32), qui n'ont pas fini sur la même commande ;
`CTRL2` et `MISO_DLEN` sont à la ligne 27. La copie vaut jusqu'au premier rangement, ou de nouveau
après une impulsion de SPI0/1, qu'inkemu fait et que QEMU ignore.

Paramètres :

```text
après reset registre 0x60002018 masque 0x00000080
après reset registre 0x60002020 masque 0x0000009F
```

### Ligne 29 — `flash_nvs.bin` : SPI1

La ligne 7, sauf `W0` après `RDSR` (`0x05`) : `nvs_rw_value_cxx` programme et efface la flash, et la
flash de QEMU garde `WEL` (le bit 1 de l'état) après la fin d'un programme ou d'un effacement, là où
une puce l'efface (`esp-idf-storage-flash-traffic.md`, résultat clé 5, et le test « WEL reads 0 after
a program » de `soc-esp32c3/src/flash.rs`). Mesure : 48 des 50 lectures de `W0` après `RDSR`
diffèrent, par les octets hauts que QEMU laisse périmés, et 16 d'entre elles aussi par `WEL` (`0x02`
chez QEMU, 0 chez inkemu). La copie fait voir
`WEL` à ESP-IDF comme sous QEMU, qui envoie alors `WRDI` et un `RDSR` de plus à chaque programme :
c'est le chemin que la copie laisse dans inkemu, et sans le lockstep inkemu envoie six commandes par
programme au lieu de neuf.

Paramètres :

```text
registre 0x60002034 masque 0x00000002
registre 0x60002058 masque 0x001860FF après 0x5A
registre 0x60002058 masque 0xFFFFFF02 après 0x05
registre 0x60002098 masque 0x00000014
registre 0x6000209C masque 0x0A002000
registre 0x600020DC masque 0x00000001
```

### Ligne 30 — `flash_nvs.bin` : EXTMEM

`ICACHE_SYNC_SIZE` (`0x600C4030`, la longueur d'une synchronisation du cache, que la ROM range avant
de la lancer autour de l'écriture de la flash, puis relit) : le cache de QEMU ne lit que
`ICACHE_CTRL`, les registres de synchronisation, de préchargement et de gel, `CACHE_STATE` et la
MMU, et rend 0 pour tout autre registre (`hw/misc/esp32c3_cache.c`, `esp32c3_cache_read`, cas par
défaut), alors qu'inkemu rend la longueur rangée. Les longueurs rangées dans la fenêtre sont 0 ou
2 Kio (`0x800`), d'où le seul bit 11. Mesure : deux lectures différentes sur 112.

Paramètres :

```text
registre 0x600C4030 masque 0x00000800
```

### Ligne 31 — `flash_nvs.bin` : ce que QEMU garde à son reset

Comme la ligne 25, avec les mêmes hypothèses ; avant son reset, `nvs_rw_value_cxx` laisse aussi
`t2`, `t4`, `t5` et `t6` différents de 0.

Paramètres :

```text
gardé au reset x1/ra reset 0x00000000
gardé au reset x2/sp reset 0x00000000
gardé au reset x3/gp reset 0x00000000
gardé au reset x4/tp reset 0x00000000
gardé au reset x5/t0 reset 0x00000000
gardé au reset x7/t2 reset 0x00000000
gardé au reset x8/s0 reset 0x00000000
gardé au reset x9/s1 reset 0x00000000
gardé au reset x11/a1 reset 0x00000000
gardé au reset x12/a2 reset 0x00000000
gardé au reset x13/a3 reset 0x00000000
gardé au reset x14/a4 reset 0x00000000
gardé au reset x15/a5 reset 0x00000000
gardé au reset x16/a6 reset 0x00000000
gardé au reset x17/a7 reset 0x00000000
gardé au reset x28/t3 reset 0x00000000
gardé au reset x29/t4 reset 0x00000000
gardé au reset x30/t5 reset 0x00000000
gardé au reset x31/t6 reset 0x00000000
gardé au reset mstatus bits 0x00000080 reset 0x00000000
gardé au reset mtvec reset 0x00000001
gardé au reset mepc reset 0x00000000
```

### Ligne 32 — `flash_nvs.bin` : SPI1 après le reset

Comme la ligne 28, avec les bits mesurés dans `nvs` : la commande de la dernière opération de la
première vie (`USER2` bas, `0x05`, `RDSR`) et un bit de phase de `USER` (bit 28, `USR_MISO`).

Paramètres :

```text
après reset registre 0x60002018 masque 0x10000080
après reset registre 0x60002020 masque 0x00000005
```

### Ligne 33 — `flash_nvs_ota.bin` : `SPI1.RD_STATUS`

Sous la table de partitions de la X4, le bootloader écrit `otadata` à chaque démarrage, et la ROM
relit ensuite l'état de la flash dans `RD_STATUS` (`0x6000202C`) : QEMU y montre `WEL` (bit 1), que
sa flash garde après le programme, là où une puce l'a effacé (comme la ligne 29). Mesure : dix
lectures différentes sur 34, sur ce seul bit, dans les deux vies.

Paramètres :

```text
registre 0x6000202C masque 0x00000002
```

### Lignes 34 à 41 — CrossPoint 1.6.5

Mesurées en local le 2026-10-14 sur la fenêtre `crosspoint` (QEMU `esp_develop_9.2.2_20250817`,
trace capturée, chaque chargement différent relevé par registre). La fenêtre s'arrête avant
l'ordonnanceur : CrossPoint touche moins de registres et de bits que `hello` dans les lignes 7, 9,
11, 14, 15, 20 et 23, et une fenêtre utilise entièrement chaque ligne qu'elle prend (décision R35
du plan). Il a donc ses propres lignes, aux registres et aux bits mesurés sur lui ; les lignes de
`hello` qu'il utilise entièrement, et la ligne 33, restent les mêmes.

### Ligne 34 — CrossPoint : SPI1

La ligne 7 ; après `RDSR` (`0x05`), `W0` diffère par les octets hauts que QEMU laisse périmés (bits
13–14 et 19–20 de la mesure) et par `WEL` (bit 1), que la flash de QEMU garde après l'écriture
d'`otadata` par le bootloader, comme dans la ligne 29. Mesure : 21 copies.

Paramètres :

```text
registre 0x60002034 masque 0x00000002
registre 0x60002058 masque 0x001860FF après 0x5A
registre 0x60002058 masque 0x00186002 après 0x05
registre 0x60002098 masque 0x00000014
registre 0x6000209C masque 0x0A002000
registre 0x600020DC masque 0x00000001
```

### Ligne 35 — CrossPoint : eFuse

`RD_SYS_PART1_DATA5` (`0x60008870`, mot 5 du bloc 2, données système), lu ici et absent de la
ligne 10 : les eFuses vierges de QEMU et ceux du profil X4 y diffèrent, comme dans la ligne 10.
Mesure : une lecture.

Paramètres :

```text
registre 0x60008870 masque 0x5ABA3A86
```

### Ligne 36 — CrossPoint : TIMG0

Les registres de la ligne 14 où la fenêtre lit une différence : le chien de garde et la calibration
de l'horloge RTC de TIMG0 ; les autres registres de la ligne 14 n'y diffèrent pas. La calibration
(`RTCCALICFG1`, `0x6001F06C`) y rend d'autres valeurs, donc d'autres bits différents, que dans
`hello`. Mesure : 7 copies.

Paramètres :

```text
registre 0x6001F048 masque 0x0004C000
registre 0x6001F068 masque 0x00008000
registre 0x6001F06C masque 0x014AE180
registre 0x6001F080 masque 0xFFFFFF99
```

### Ligne 37 — CrossPoint : RTC_CNTL

La ligne 9, l'horloge lente comprise ; dans `WDTCONFIG0` (`0x60008090`), les bits 27–29 ne
diffèrent pas dans cette fenêtre et ne sont pas pris. Mesure : 86 usages.

Paramètres :

```text
horloge lente 0x60008010 capture 0x6000800C bit 31 136000 Hz écart 1 qemu 0x00000000
registre 0x60008000 masque 0x1C00A000
registre 0x6000801C masque 0x3C140543
registre 0x60008024 masque 0x0A080A09
registre 0x60008028 masque 0x12200A09
registre 0x6000802C masque 0x00008000
registre 0x60008030 masque 0x0A080000
registre 0x60008034 masque 0x00D40000
registre 0x60008038 masque 0x00003000
registre 0x60008040 masque 0x00000200
registre 0x60008070 masque 0x31D83298
registre 0x60008074 masque 0x00400000
registre 0x60008080 masque 0xA03FC000
registre 0x60008088 masque 0x00575010
registre 0x6000808C masque 0xBA805480
registre 0x60008090 masque 0xC007FA14
registre 0x600080AC masque 0x04B00000
registre 0x600080D8 masque 0x7BFFC030
registre 0x6000810C masque 0x00000007
registre 0x6000811C masque 0xC0000000
```

### Ligne 38 — CrossPoint : IO_MUX

Comme la ligne 11, sur les seules broches que la fenêtre lit : celles de la flash (GPIO12–17) et de
l'UART0 (GPIO20–21). Le bit 10 (`FUN_DRV`) que la ligne 11 prend sur GPIO12–17 ne diffère pas
ici. Mesure : 64 copies.

Paramètres :

```text
registre 0x60009034 masque 0x00001B00
registre 0x60009038 masque 0x00001B00
registre 0x6000903C masque 0x00000B00
registre 0x60009040 masque 0x00000B00
registre 0x60009044 masque 0x00000B00
registre 0x60009048 masque 0x00000B00
registre 0x60009054 masque 0x00000B00
registre 0x60009058 masque 0x00000B00
```

### Ligne 39 — CrossPoint : le compteur 0 du SYSTIMER

La ligne 15 pour le seul compteur que la fenêtre lit, le compteur 0 (le compteur 1 sert au tick de
FreeRTOS, qui ne démarre pas avant la fin de la fenêtre), et le bit `VALUE_VALID` de `UNIT0_OP`.
Mesure : 13 usages, mouvement maximal de l'écart 1.

Paramètres :

```text
registre 0x60023004 masque 0x20000000
compteur 0x60023044 recul par mise à jour 0x60023000-0x600230FF origines 0x600C0018 0x60023000
```

### Ligne 40 — CrossPoint : SYSTEM

La ligne 20, sans le bit 13 de `PERIP_RST_EN0` (`TIMERGROUP_RST`), égal des deux côtés dans cette
fenêtre. Mesure : 35 copies.

Paramètres :

```text
registre 0x600C0000 masque 0x00000040
registre 0x600C0004 masque 0x000000C0
registre 0x600C0008 masque 0x0000000C
registre 0x600C000C masque 0x00000001
registre 0x600C0010 masque 0xF9C1E06F
registre 0x600C0014 masque 0x00000204
registre 0x600C0018 masque 0xA86D9BEE
registre 0x600C001C masque 0x000001FE
registre 0x600C0020 masque 0x000000FF
registre 0x600C0024 masque 0x02001001
registre 0x600C0040 masque 0x00000007
registre 0x600C0058 masque 0x00080401
```

### Ligne 41 — CrossPoint : ASSIST_DEBUG

Trois registres de la ligne 23, que le bootloader et le démarrage de l'application touchent avec la
même valeur des deux côtés : ils sont admis, rien n'est copié. Les autres registres de la ligne 23
n'y diffèrent pas ou n'y sont pas touchés. Mesure : 7 accès.

Paramètres :

```text
registre 0x600CE044 masque 0x00000000
registre 0x600CE048 masque 0x00000000
registre 0x600CE098 masque 0x00000000
```

## Pourquoi la ligne 24 n'est pas une resynchronisation

Aucune lecture de registre n'explique l'interruption précoce, donc rien à copier ; faire exécuter à
inkemu seul son gestionnaire jusqu'au retour à l'instruction interrompue laisse `mepc` et `mcause`
différents jusqu'au piège suivant de QEMU, et le gestionnaire de tick lit un compteur dont QEMU ne
donne pas la valeur. Le réglage reproduit le comportement de QEMU dans la machine, pour le lockstep
seulement.

## Ce qui n'est pas comparé

- Les CSR que QEMU n'imprime pas ou qu'inkemu n'a pas sous ce nom : PMP, `mcountinhibit`, `mpcer`,
  `mpcmr`, sinon à travers leurs lectures dans un registre (ligne 26 pour la PMP). Un rangement n'est
  comparé qu'à travers les chargements qui le relisent.
- Un bloc qu'une interruption a préempté (ci-dessous) : l'état qu'il porte est comparé au premier
  bloc du gestionnaire, `mepc` et `mcause` compris.
- Ce que les copies de valeurs et les réglages remplacent (moniteur de pile, changement d'horloge,
  interruption de tick précoce) : chaque section le dit.
- L'origine d'un compteur (lignes 5 et 15) : la première lecture après une origine fixe l'écart, et
  seule son évolution est tenue. Un compteur d'inkemu décalé d'une constante n'est pas vu.

## Ce qui n'est pas une différence

Deux sortes de blocs de la trace de QEMU ne sont pas des états d'instruction et sont écartés (ADR
0008) : un bloc réimprimé à l'identique quand le budget d'instructions de QEMU se recharge (100 dans
`hello`, 114 dans `nvs`, 657 dans `crosspoint`, jamais deux de suite ; un deuxième à l'identique est comparé comme une
instruction exécutée, sauf un saut sur lui-même reconnu sur ses bits et l'état de QEMU), et un bloc
dont QEMU prend l'interruption avant d'exécuter l'instruction (20 dans chacune des fenêtres de la
CI, aucun dans `crosspoint`). Le temps est aligné par le modèle
`qemu-icount` (décision H2), pas par une ligne.
