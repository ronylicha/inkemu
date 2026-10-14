# Différences admises entre inkemu et QEMU (lockstep)

Le lockstep (`inkemu-lockstep`, crate `lockstep`, ADR 0008) compare après chaque instruction le PC,
`x1`–`x31` et les huit CSR machine que les deux côtés ont (`mstatus`, `mie`, `mip`, `mtvec`,
`mepc`, `mcause`, `mtval`, `mscratch`). Toute différence qui n'est pas une ligne de ce tableau est
une divergence (décision H3). Deux contrôles tiennent ce tableau :

- **Le tableau de l'outil est celui-ci.** `lockstep::DIFFERENCES` (`crates/lockstep/src/table.rs`)
  et ce document ont les mêmes lignes, les mêmes traitements et les mêmes paramètres, registre par
  registre et bit par bit : `crates/lockstep/tests/table.rs` compare chaque bloc « Paramètres »
  ci-dessous à la ligne de l'outil.
- **Chaque ligne sert, et seulement dans ce qu'elle déclare.** L'outil enregistre ce que chaque ligne
  a fait pendant la fenêtre (les registres, les bits pris à QEMU, l'écart d'un compteur, ce qu'un
  réglage a changé) ; une ligne jamais utilisée, ou utilisée hors de ses registres ou de ses bits,
  fait échouer l'exécution (code de sortie 5, `lockstep::Usage::check`). L'outil imprime cet usage
  à chaque exécution, la CI aussi.

QEMU : `esp_develop_9.2.2_20250817`, celui de l'image `espressif/idf:v5.5.2`
(`tools/conformance/versions.env`). Les lignes ont été mesurées le 2026-10-14 sur
`flash_hello.bin` (ROM, bootloader, application jusqu'à `esp_restart`, 3 402 169 instructions
comparées) ; chaque masque est l'ensemble des bits mesurés différents, rien de plus. Un firmware qui
montre un autre bit, ou touche un autre registre, échoue jusqu'à ce qu'une ligne l'admette avec sa
preuve.

## La fenêtre (H4, corrigée le 2026-10-14)

La comparaison s'arrête, sans divergence, au premier reset des deux côtés à la fois, et au premier
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

## Traitements, par ordre de préférence (H3)

- **(i) paire de valeurs** : des bits d'un CSR comparé valent exactement une valeur dans QEMU et une
  autre dans inkemu ; les autres bits sont comparés, et une lecture du CSR dans un registre est tenue
  à la même paire. Une paire « jusqu'à écriture » ne vaut que jusqu'à ce qu'une instruction écrive le
  CSR. Le bit `MEIP` de `mip` (ligne 4) est tenu à la ligne que la matrice de QEMU calcule dans l'état
  du contrôleur d'inkemu.
- **(ii) copie de bits** : après un chargement d'un mot depuis un registre nommé, le registre
  destination d'inkemu prend les bits du masque tels que QEMU les a chargés, les autres bits devant
  être égaux. Un registre de masque 0 est un accès admis à un registre que QEMU n'a pas : rien n'est
  copié, la valeur est comparée. Une copie peut porter une condition (la commande SPI1 qui a précédé)
  et une valeur que QEMU doit avoir lue. **(ii) écart borné** : un compteur que QEMU et inkemu lisent
  à au plus quelques unités l'un de l'autre prend la valeur de QEMU ; un écart plus grand est une
  divergence.
- **(iii) resynchronisation** à un PC d'ancrage : aucune ligne n'en a eu besoin.
- **(iv) réglage `QemuCompat`** de la machine, désactivé par défaut, que seul le lockstep allume,
  quand la différence change le flot de contrôle et qu'aucun des trois premiers ne suffit. La ligne
  compte une utilisation chaque fois que le réglage a changé quelque chose (la machine le compte).

## Tableau

La colonne « Usages » donne ce que chaque ligne a fait dans la fenêtre de `flash_hello.bin` : une
comparaison qu'une paire règle, une valeur copiée, un accès admis, une action d'un réglage.

| N° | Différence | Traitement | Source | Usages |
|---|---|---|---|---|
| 1 | `mstatus.MPP` : QEMU lit 0, inkemu « machine » | (i) paire | `crates/cpu-rv32/src/csr.rs` (`MSTATUS_MPP_M`) ; mesure | 2 781 658 |
| 2 | `mie` au reset : QEMU `0x0EEE`, inkemu 0, jusqu'à ce que le firmware l'écrive | (i) paire | oracle §1c piège 6 ; `target/riscv/esp_cpu.c` | 3 402 170 |
| 3 | `mtvec.MODE` au reset : QEMU direct, inkemu vectorisé, jusqu'à l'écriture de `mtvec` | (i) paire | `crates/cpu-rv32/src/csr.rs` (`MTVEC_VECTORED`) ; oracle §1c | 15 |
| 4 | `mip.MEIP` : la ligne de la matrice de QEMU, trois instructions en retard après un rangement du seuil | (i) paire | `hw/riscv/esp32c3_intmatrix.c` ; `target/riscv/esp_cpu.c` ; mesure | 94 494 |
| 5 | Le compteur de cycles : celui de QEMU compte 6,25 ns de son horloge depuis la première lecture | (iv) réglage ; (ii) écart borné | `target/riscv/esp_cpu.c` (`esp_cpu_get_cycles`) ; mesure | 532 149 |
| 6 | UART0 : niveaux de ligne, compte de FIFO et valeurs de reset que QEMU ne modélise pas | (ii) copie | mesure ; `info mtree -f` (`esp_soc.uart`) | 3 165 |
| 7 | SPI1 : la flash de QEMU ne répond pas à SFDP, laisse `W0` périmé et n'a pas les valeurs de reset | (ii) copie | oracle §1b (`Unknown cmd 5a`) ; mesure | 20 |
| 8 | SPI0, absent de QEMU : la configuration flash du cache | (ii) copie | `info mtree -f` ; mesure | 81 |
| 9 | RTC_CNTL : QEMU ne modélise que `OPTIONS0`, `RESET_STATE` et `STORE0`–`7` | (ii) copie | `hw/misc/esp32c3_rtc_cntl.c` ; mesure | 92 |
| 10 | eFuse : les mots où les eFuses vierges de QEMU et ceux de la X4 diffèrent | (ii) copie | oracle §1d ; carryover SP1b-2 ligne 17 ; mesure | 67 |
| 11 | IO_MUX, absent de QEMU : la configuration des broches GPIO0–21 | (ii) copie | `info mtree -f` ; SVD ; mesure | 284 |
| 12 | Le maître I²C analogique, absent de QEMU : les accès regi2c de la ROM | (ii) copie | carryover SP1b-2 lignes 7 et 8 ; mesure | 147 |
| 13 | La mise sous tension de la radio par `rtc_sleep_pu`, absente de QEMU : BB, NRX, FE, FE2 | (ii) copie | ESP-IDF v5.5.2 `rtc_sleep.c` ; `esp32c3-bootloader-accesses.md` lignes 31–33 | 14 |
| 14 | TIMG0 et TIMG1 : valeurs de reset des chiens de garde, portes d'horloge, calibration RTC | (ii) copie | `timers-and-watchdogs` §G lignes 1, 5 et 8 ; mesure | 13 |
| 15 | SYSTIMER : les compteurs de QEMU, tronqués à chaque mise à jour et jamais remis à zéro par SYSTEM | (iv) réglage ; (ii) écart borné | `hw/timer/esp_systimer.c` ; mesure | 2 228 |
| 16 | APB_CTRL, absent de QEMU : horloges Wi-Fi et alimentation des mémoires | (ii) copie | `info mtree -f` ; mesure | 21 |
| 17 | `RNG.DATA` : QEMU rend le `rand()` de l'hôte | (ii) copie | `hw/riscv/esp32c3.c` (`A_SYSCON_RND_DATA_REG`) | 1 604 |
| 18 | APB_SARADC, absent de QEMU : les sept registres de la source d'entropie du bootloader | (ii) copie | ESP-IDF v5.5.2 `bootloader_random_esp32c3.c` ; `esp32c3-bootloader-accesses.md` lignes 79–85 | 48 |
| 19 | USB-Serial-JTAG : `SERIAL_IN_EP_DATA_FREE`, que QEMU lit 0 | (ii) copie | carryover SP1b-2 ligne 22 ; mesure | 7 033 |
| 20 | SYSTEM : portes d'horloge, resets et choix d'horloge que le modèle d'horloge de QEMU tient autrement | (ii) copie | `timers-and-watchdogs` §G ligne 12 ; mesure | 46 |
| 21 | SENSITIVE, absent de QEMU : la protection mémoire qu'ESP-IDF installe | (ii) copie | `info mtree -f` ; mesure | 77 |
| 22 | EXTMEM : bits d'alimentation, de gel et de synchronisation que le cache de QEMU ne tient pas | (ii) copie | carryover SP1b-2 ligne 3 ; mesure | 24 |
| 23 | ASSIST_DEBUG, absent de QEMU : le moniteur de pile de FreeRTOS | (ii) copie | `timers-and-watchdogs` §G ligne 13 ; mesure | 14 447 |
| 24 | SYSTIMER : pas d'alarme tant que le compteur est arrêté | (iv) réglage | `timers-and-watchdogs` §A.4 et §G ligne 10 ; `hw/timer/esp_systimer.c` | 1 |

Les sources de QEMU sont celles de la branche `esp-develop` d'`espressif/qemu`. « Mesure » renvoie à
la mesure du 2026-10-14 : chaque registre de la fenêtre lu ou écrit, les valeurs des deux côtés à
chaque chargement, avec le PC (relevé décrit dans la note de passation de SP1b-4c).

## Paramètres et justification, ligne par ligne

Chaque bloc « Paramètres » est la ligne de l'outil, dans la syntaxe que le test lit : `paire <CSR>
bits <masque> qemu <valeur> inkemu <valeur> [jusqu'à écriture]`, `meip retard <instructions>`,
`registre <adresse> masque <bits> [après <commande SPI1>] [qemu <valeur>]`, `compteur <adresse>
écart <unités>` ou `compteur csr <numéro> écart <unités>`, `réglage <nom du champ de QemuCompat>`.

### Ligne 1 — `mstatus.MPP`

inkemu est un cœur à mode machine seul : `MPP` se lit toujours « machine » (`0b11`). QEMU lit 0 dans
toute la fenêtre, au reset comme après chaque `mret`. La paire tient chaque comparaison et chaque
lecture de `mstatus` dans un registre ; un autre bit de `mstatus` qui différerait est une divergence.

Paramètres :

```text
paire mstatus bits 0x00001800 qemu 0x00000000 inkemu 0x00001800
```

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
identifié ; la ligne l'admet pour trois instructions après un tel rangement, pas une de plus.

Paramètres :

```text
meip retard 3
```

### Ligne 5 — le compteur de cycles

QEMU garde un compteur par adresse (`0x7E2`, `0x802`) qui compte des cycles de 6,25 ns de son horloge
virtuelle à partir de sa première lecture, reporte le reste de la division, et ignore les registres
d'événement et de mode ; une écriture fixe la valeur sans toucher l'instant de la dernière lecture,
et `csrw` ne lit pas (`esp_cpu_get_cycles`, `esp_cpu_csr_write`). Le réglage
`cycle_counter_as_qemu` fait compter ainsi le compteur d'inkemu, sur le temps de l'appareil. Reste
un écart d'un cycle au plus : QEMU compte 8 ns exactement par instruction, inkemu en cycles entiers de
la base de temps (`floor(n × 32 / 25)`). Mesure : `0x7E2` −1 (1 138 lectures), 0 (505), +1 (6) ;
`0x802` −1 ou 0 sur 200 000 lectures. Avant le réglage, l'écart croissait sans borne (inkemu comptait
ses propres cycles, pas le temps). Les usages comptent les 283 221 lectures que le réglage a servies
et les 248 928 valeurs copiées dans la borne.

Paramètres :

```text
réglage cycle_counter_as_qemu
compteur csr 0x7E2 écart 1
compteur csr 0x802 écart 1
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
`RESET_STATE`, que QEMU ne met pas ; la cause du reset est comparée. `TIME_LOW0` : QEMU lit toujours
0, ce que la ligne exige ; le temporisateur RTC d'inkemu n'est pas comparé. Les autres registres :
valeurs de reset, minuteries d'alimentation, horloges et chien de garde RTC qu'inkemu tient et que
QEMU lit 0.

Paramètres :

```text
registre 0x60008000 masque 0x1C00A000
registre 0x60008010 masque 0xFFFFFFFF qemu 0x00000000
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
registre 0x600080AC masque 0x04B00000
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

Le SYSTIMER de QEMU (`hw/timer/esp_systimer.c`) tronque un compte à l'unité chaque fois qu'il met un
compteur à jour (lecture `UPDATE`, chargement, alarme, reprogrammation d'un comparateur) et reprend
la base à l'instant présent, si bien qu'il perd en moyenne une demi-unité par mise à jour ; il met à
jour un compteur arrêté quand il reprogramme un comparateur qui le suit ; `SYSTEM.PERIP_RST_EN0` ne le
remet jamais à zéro ; `VALUE_VALID` se lit toujours 1. Le réglage `systimer_counts_as_qemu` fait lire à
inkemu ces valeurs-là, ses alarmes restant celles de ses propres compteurs (elles tombent aux mêmes
instructions que celles de QEMU dans toute la fenêtre). Reste, sur le compteur 0, un écart d'au plus
deux unités : QEMU arrondit le délai d'une alarme à la microseconde (`TICKS_TO_NS`) et date ses mises
à jour sur une grille de 8 ns, inkemu sur la base de temps de 6,25 ns. Mesure : compteur 1 égal sur
2 200 lectures ; compteur 0 en avance d'une unité sur 12 lectures, de deux sur 2. Avant le réglage,
QEMU avait 293 412 à 303 246 unités d'avance (19 ms : le reset de SYSTEM que QEMU ignore, la mise à
jour d'un compteur arrêté) et l'écart dérivait d'une unité par période de tick. Les usages comptent
les 2 214 actions du réglage (resets ignorés, lectures rendues autrement) et les 14 valeurs copiées
dans la borne.

Paramètres :

```text
réglage systimer_counts_as_qemu
compteur 0x60023044 écart 2
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

QEMU rend `rand()` de l'hôte, semé par `srand(time(NULL))` (`hw/riscv/esp32c3.c`) : aucune option de
QEMU ne le rend reproductible, et ce n'est pas un compteur. Le mot entier est copié. Le flot du
bootloader n'en dépend pas dans la fenêtre : deux exécutions vivantes comparent le même nombre
d'instructions.

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

Le modèle `esp32c3.soc.clk` de QEMU ne tient pas les portes d'horloge et les resets des périphériques
comme le composant, et `SYSCLK_CONF` y lit la PLL sélectionnée. La copie laisse inkemu dans un état que
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
registre 0x600C0018 masque 0xA86DBBEE
registre 0x600C001C masque 0x000001FE
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

## Pourquoi la ligne 24 n'est pas une resynchronisation

Aucune lecture de registre n'explique l'interruption précoce, donc rien à copier ; faire exécuter à
inkemu seul son gestionnaire jusqu'au retour à l'instruction interrompue laisse `mepc` et `mcause`
différents jusqu'au piège suivant de QEMU, et le gestionnaire de tick lit un compteur dont QEMU ne
donne pas la valeur. Le réglage reproduit le comportement de QEMU dans la machine, pour le lockstep
seulement.

## Ce qui n'est pas comparé

- Les CSR que QEMU n'imprime pas ou qu'inkemu n'a pas sous ce nom : PMP, `mcountinhibit`, `mpcer`,
  `mpcmr`. Un rangement n'est comparé qu'à travers les chargements qui le relisent.
- Un bloc qu'une interruption a préempté (ci-dessous) : l'état qu'il porte est comparé au premier
  bloc du gestionnaire, `mepc` et `mcause` compris.
- Ce que les copies de valeurs et les réglages remplacent (temporisateur RTC, moniteur de pile,
  changement d'horloge, interruption de tick précoce) : chaque section le dit.

## Ce qui n'est pas une différence

Deux sortes de blocs de la trace de QEMU ne sont pas des états d'instruction et sont écartés (ADR
0008) : un bloc réimprimé à l'identique quand le budget d'instructions de QEMU se recharge (50 dans la
fenêtre, jamais deux de suite ; un deuxième à l'identique est comparé comme une instruction exécutée,
sauf un saut sur lui-même reconnu sur ses bits et l'état de QEMU), et un bloc dont QEMU prend
l'interruption avant d'exécuter l'instruction (10 dans la fenêtre). Le temps est aligné par le modèle
`qemu-icount` (décision H2), pas par une ligne.
