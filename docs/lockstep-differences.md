# Différences admises entre inkemu et QEMU (lockstep)

Le lockstep (`inkemu-lockstep`, crate `lockstep`, ADR 0008) compare après chaque instruction le PC,
`x1`–`x31` et les CSR machine que QEMU imprime (`mstatus`, `mie`, `mip`, `mtvec`, `mepc`, `mcause`,
`mtval`, `mscratch`). Toute différence qui n'est pas une ligne de ce tableau est une divergence
(décision H3). Ce tableau et `lockstep::DIFFERENCES` (`crates/lockstep/src/table.rs`) sont le même
tableau : `crates/lockstep/tests/table.rs` vérifie qu'ils ont les mêmes lignes et les mêmes
traitements.

QEMU : `esp_develop_9.2.2_20250817`, celui de l'image `espressif/idf:v5.5.2`. Les lignes ont été
établies le 2026-10-14 sur `flash_hello.bin` (ROM, bootloader, application jusqu'à `esp_restart`,
3 402 169 instructions) ; la colonne « Usages » est le nombre de fois où la ligne a servi dans cette
fenêtre : une comparaison qu'un masque règle, une valeur recopiée, un accès admis à un bloc absent ;
une ligne (iv) compte une fois, pour la machine construite avec son réglage.

## Traitements, par ordre de préférence (H3)

- **(i) masque** : des bits d'une grandeur comparée sont laissés hors de la comparaison.
- **(ii) recopie** : après un chargement depuis une plage de registres nommée (ou la lecture d'un
  CSR nommé), le registre destination d'inkemu prend la valeur que QEMU a chargée, seulement quand
  elles diffèrent. Une ligne (ii) peut couvrir des registres d'un bloc que QEMU n'émule pas : ces
  accès ne terminent alors pas la fenêtre (H4), parce que la ligne dit ce que le firmware y fait et
  pourquoi les zéros de QEMU ne le changent pas ; tout autre accès au bloc la termine.
- **(iii) resynchronisation** à un PC d'ancrage : aucune ligne n'en a eu besoin.
- **(iv) réglage `QemuCompat`** de la machine, désactivé par défaut, que seul le lockstep allume,
  quand la différence change le flot de contrôle et qu'aucun des trois premiers ne suffit.

## Tableau

| N° | Différence | Traitement | Grandeur ou registres | Source | Usages |
|---|---|---|---|---|---|
| 1 | `mstatus.MPP` se lit « machine » dans inkemu, qui n'a pas de mode utilisateur ; le cœur générique de QEMU garde ce qui est écrit et `mret` y met U | (i) masque ; (ii) recopie | bits [12:11] de `mstatus`, et de la lecture de `mstatus` | `crates/cpu-rv32/src/csr.rs` (`MSTATUS_MPP_M`) ; spécification privilégiée RISC-V §3.1.6.1 | 2 781 658 |
| 2 | `mie` vaut `0x0eee` au reset dans QEMU, 0 dans inkemu ; le firmware ne l'écrit pas (l'ESP32-C3 active ses lignes dans la matrice d'interruptions) | (i) masque | bits `0x0eee` de `mie` | `docs/research/sp1b-test-oracle.md` §1c, piège 6 ; décision E3 (`InterruptModel::Controller`) | 3 402 170 |
| 3 | `mtvec.MODE` est câblé à « vectorisé » dans inkemu ; QEMU part de 0 | (i) masque | bit 0 de `mtvec` | `crates/cpu-rv32/src/csr.rs` (`MTVEC_VECTORED`) ; oracle §1c (bloc de reset) | 15 |
| 4 | Le cœur de QEMU met `mip.MEIP` tant que sa matrice présente une ligne ; dans inkemu `mip` est un registre simple | (i) masque | bit 11 de `mip` | oracle §1c (`mip 00000800` après `cause:0000000b desc=m_external`) ; décision E3 | 94 494 |
| 5 | Le compteur de performance compte les cycles d'inkemu et ceux de QEMU, qui ne le remet pas à zéro au reset | (ii) recopie | CSR `0x7E2` (`mpccr`) et `0x802` | `docs/research/esp32c3-timers-and-watchdogs.md` §G ligne 13 | 283 220 |
| 6 | UART0 : la FIFO d'émission de QEMU se vide aussitôt (`STATUS` lu 0) et `CONF0`, `FSM_STATUS`, `CLK_CONF` ne sont pas ceux du composant ; celle d'inkemu se vide au débit de la ligne | (ii) recopie | `UART0.STATUS`, `CONF0` (`0x6000_001C`–`0x6000_0020`), `FSM_STATUS` (`0x6000_006C`), `CLK_CONF` (`0x6000_0078`) | `docs/research/sp1b-4-scoping.md` §0 point 2 ; `info mtree -f` (`esp_soc.uart`) | 3 165 |
| 7 | SPI1 : QEMU lit 0 dans `MISC`, `FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL`, `CLOCK_GATE` ; sa flash rejette la lecture SFDP (`0x5A`), `W0` garde alors une donnée antérieure où inkemu répond `0xFF` | (ii) recopie | `SPI1.MISC` (`0x6000_2034`), `W0` (`0x6000_2058`), `FLASH_WAITI_CTRL`–`FLASH_SUS_CTRL` (`0x6000_2098`–`0x6000_209C`), `CLOCK_GATE` (`0x6000_20DC`) | `docs/superpowers/notes/2026-10-11-sp1b-2-carryover.md` ligne 9 ; oracle §1b (`Unknown cmd 5a`) | 20 |
| 8 | SPI0 n'est pas un périphérique dans QEMU : ses registres se lisent 0 | (ii) recopie | `SPI0`, `0x6000_3000`–`0x6000_33FF` | carryover ligne 21 ; `info mtree -f` (zone `esp32c3.iomem`) | 26 |
| 9 | RTC_CNTL : QEMU ne modélise que `OPTIONS0`, `RESET_STATE` (la seule cause) et `STORE0`–`7` ; le reste se lit 0 | (ii) recopie | `RTC_CNTL`, `0x6000_8000`–`0x6000_81FF` sauf `STORE0`–`3` (`0x6000_8050`–`0x6000_805F`) et `STORE4`–`7` (`0x6000_80B8`–`0x6000_80C7`) | timers §G lignes 2 et 13 | 92 |
| 10 | eFuse : la puce de QEMU a un bloc v1.3, ni MAC ni calibration ; celle d'inkemu est une v0.3 de production, bloc v1.2 | (ii) recopie | `EFUSE`, `0x6000_8800`–`0x6000_89FF` | carryover ligne 17 ; oracle §1d | 67 |
| 11 | IO_MUX n'est pas un périphérique dans QEMU : ses registres se lisent 0 | (ii) recopie | `IO_MUX`, `0x6000_9000`–`0x6000_90FF` | `info mtree -f` (zone `esp32c3.iomem`) | 40 |
| 12 | Le maître I²C analogique (regi2c) se lit `0xFFFFFF` dans QEMU | (ii) recopie | `0x6000_E000`–`0x6000_E0FF` | carryover lignes 7 et 8 | 79 |
| 13 | BB, que QEMU n'émule pas : `rtc_sleep_pu` y fait une lecture-modification-écriture au démarrage de l'application, sans rien en attendre | (ii) recopie, accès admis | `0x6001_D054` | `info mtree -f` ; ESP-IDF v5.5.2 `components/esp_hw_support/port/esp32c3/rtc_sleep.c` (`rtc_sleep_pu`) ; mesure locale | 4 |
| 14 | TIMG0/1 : QEMU met à 0 la configuration des chiens de garde au reset, calcule la calibration autrement et lit `REGCLK` 0 | (ii) recopie | `WDTCONFIG0`–`1`, `RTCCALICFG`–`RTCCALICFG1`, `RTCCALICFG2`, `REGCLK` de TIMG0 ; `WDTCONFIG0`–`1`, `REGCLK` de TIMG1 | timers §G lignes 1, 5 et 8 | 13 |
| 15 | SYSTIMER : `VALUE_VALID` se lit toujours 1 dans QEMU, et ses compteurs partent du démarrage de la machine | (ii) recopie | `UNIT0_OP`–`UNIT1_OP` (`0x6002_3004`–`0x6002_3008`), `UNIT0_VALUE_HI`–`UNIT1_VALUE_LO` (`0x6002_3040`–`0x6002_304C`) | timers §G ligne 9 | 2 220 |
| 16 | APB_CTRL n'est pas un périphérique dans QEMU : `WIFI_CLK_EN` et les registres d'alimentation des mémoires se lisent 0 | (ii) recopie | `APB_CTRL.WIFI_CLK_EN` (`0x6002_6014`), `FRONT_END_MEM_PD`–`MEM_POWER_UP` (`0x6002_609C`–`0x6002_60AC`) | `info mtree -f` (zone `esp32c3.iomem`) | 4 |
| 17 | `RNG.DATA` : QEMU tire l'aléa de l'hôte, inkemu d'une graine fixe | (ii) recopie | `0x6002_60B0` | carryover ligne 20 ; `docs/research/esp32c3-bootloader-accesses.md` §10 point 5 | 1 604 |
| 18 | APB_SARADC, que QEMU n'émule pas : la source d'entropie du bootloader le programme puis l'arrête, sans lire de mesure | (ii) recopie, accès admis | `APB_SARADC`, `0x6004_0000`–`0x6004_0057` | `info mtree -f` ; ESP-IDF v5.5.2 `components/bootloader_support/src/bootloader_random_esp32c3.c` ; mesure locale | 67 |
| 19 | `USB_DEVICE.EP1_CONF` se lit 0 dans QEMU : la ROM attend une place qui ne vient pas, puis renonce | (ii) recopie | `0x6004_3004` | carryover ligne 22 | 7 033 |
| 20 | SYSTEM : QEMU lit 0 dans les portes d'horloge et les resets des périphériques, et `SYSCLK_CONF` sur la PLL | (ii) recopie | `SYSTEM`, `0x600C_0000`–`0x600C_0027`, `CACHE_CONTROL` (`0x600C_0040`), `SYSCLK_CONF` (`0x600C_0058`) | timers §G ligne 12 ; carryover ligne 6 | 46 |
| 21 | SENSITIVE n'est pas un périphérique dans QEMU : ses registres se lisent 0 | (ii) recopie | `SENSITIVE`, `0x600C_1000`–`0x600C_1FFF` | `info mtree -f` (zone `esp32c3.iomem`) | 17 |
| 22 | EXTMEM : les registres de contrôle, d'alimentation, de synchronisation et de gel du cache de QEMU se lisent autrement | (ii) recopie | `ICACHE_CTRL1`–`ICACHE_TAG_POWER_CTRL` (`0x600C_4004`–`0x600C_4008`), `ICACHE_SYNC_CTRL` (`0x600C_4028`), `CACHE_MMU_POWER_CTRL` (`0x600C_40AC`), `CACHE_MMU_OWNER` (`0x600C_40C4`), `ICACHE_FREEZE` (`0x600C_40CC`) | carryover ligne 3 ; `info mtree -f` (`esp32c3.cache`) | 24 |
| 23 | ASSIST_DEBUG n'est pas un périphérique dans QEMU : `CORE_0_MONTR_ENA`, lu par le moniteur de pile à chaque entrée d'interruption, se lit 0 | (ii) recopie | `0x600C_E000` | timers §G ligne 13 | 2 222 |
| 24 | Le SYSTIMER de QEMU ne compare qu'un compteur qui tourne : le comparateur que `vSystimerSetup` active sur le compteur 1 arrêté ne donne pas la première interruption de tick précoce qu'inkemu donne | (iv) `QemuCompat::systimer_no_alarm_on_stopped_counter` | SYSTIMER, comparateur en mode cible | timers §A.4 et §G ligne 10 ; hypothèse de `Systimer::evaluate` | 1 |

## Pourquoi la ligne 24 n'est pas une resynchronisation

Le masque ne cache pas une interruption ; aucune lecture de registre ne l'explique, donc rien à
recopier ; faire exécuter à inkemu seul son gestionnaire jusqu'au retour à l'instruction
interrompue laisse `mepc` et `mcause` différents jusqu'au piège suivant de QEMU, et le gestionnaire
de tick lit un compteur dont QEMU ne donne pas la valeur. Le réglage reproduit le comportement de
QEMU dans la machine, pour le lockstep seulement.

## Ce qui n'est pas une différence

Deux sortes de blocs de la trace de QEMU ne sont pas des états d'instruction et sont écartés par le
lecteur (ADR 0008) : un bloc réimprimé à l'identique quand le budget d'instructions de QEMU se
recharge (50 dans la fenêtre), et un bloc dont QEMU prend l'interruption avant d'exécuter
l'instruction (10 dans la fenêtre). Le temps est aligné par le modèle `qemu-icount` (décision H2),
pas par une ligne.
