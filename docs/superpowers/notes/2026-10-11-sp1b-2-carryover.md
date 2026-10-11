# SP1b-2 — décisions prises et points reportés

Bilan de l'exécution du plan `docs/superpowers/plans/2026-10-11-sp1b-2-flash-bootloader.md`. Ce document garde deux choses que l'historique git ne dit pas : les **décisions** prises en cours de route, et ce qui est **volontairement reporté**, avec le plan qui en hérite.

Il est écrit par la tâche 17, la dernière du plan. Les tâches 0 à 15 ont été relues une à une ; la relecture de la tâche 16 et celle de la **branche entière** n'ont pas encore eu lieu. La section « Décisions prises pendant l'exécution » se limite à ce que les comptes rendus de tâche et le code permettent de vérifier ; le contrôleur la complétera avec les suites de ces relectures.

## Résultat

Avec l'image flash de référence (SHA-256 `fc05ef5dbe85372e1da52abdec3bfcbbb110738a60163734666095fcd8c39bf2`, 16 Mio : le bootloader ESP-IDF v5.5.2 et l'exemple `hello_world`, construits par `tools/reference-firmware/build.sh`), `inkemu run --device x4 --flash <image>` affiche les lignes de la ROM sur son chemin de démarrage depuis la flash, les 21 lignes du bootloader de second niveau, puis la première ligne de l'application (`I (31) cpu_start: Unicore app`), et rend le code de sortie 0 à la limite de `--max-cycles`. Il reste alors trois diagnostics distincts, tous sur la calibration de l'horloge lente du RTC (`TIMG0.RTCCALICFG` lu, `RTCCALICFG2` lu et écrit) : l'application attend cette calibration, et l'exécution ne s'arrête que sur `--max-cycles`.

En mode strict, aucun registre non modélisé n'est touché et aucune exception CPU n'est levée du reset jusqu'à la première ligne de l'application ; l'exécution s'arrête (code de sortie 3) sur la lecture de `TIMG0.RTCCALICFG` au PC `0x403829fa`, après 1 937 072 cycles (32 068 µs de temps d'appareil, CPU à 80 MHz). C'est le point de départ du plan SP1b-3.

Le job CI `reference-boot`, qui reconstruit l'image dans Docker et rejoue ces tests, n'a pas encore tourné sur GitHub.

## Décisions prises pendant l'exécution

| # | Décision | Raison | Coût si elle est fausse |
|---|---|---|---|
| R1 | `sha2`, `md-5` et `sha1` passent à la ligne 0.11 dès la tâche 0, au lieu de la 0.10 prévue par D20. La sortie d'un condensat n'implémente plus `LowerHex` : chaque crate qui en affiche un a un petit utilitaire `hex`. | La règle « toujours la dernière version stable » ; reporter un bump connu est une dette. | Une tâche de plus au début. |
| R2 | Les tests `#[ignore]` sur la vraie ROM sont rouges au commit de la tâche 6 et de nouveau verts à celui de la tâche 7. | La table MMU (tâche 6) doit exister avant les fenêtres (tâche 7) ; les fusionner donnait une unité de relecture trop grosse. | Un commit où une bissection avec la vraie ROM est rouge. |
| R3 | Un accès à travers une entrée MMU invalide signale l'adresse de l'accès, pas une adresse par octet. | Le diagnostic nomme l'accès que l'utilisateur reconnaît, pas un octet à l'intérieur. | Nul sur l'adresse. Limite connue : un chargement de 4 octets à travers une même entrée invalide enregistre encore le diagnostic une fois par octet, si bien que le « répété N fois » compte des octets et non des accès. |
| R4 | L'assembleur d'image flash complète des tests, `build_flash_image`, vit dans `firmware::builder` (tâche 14) et sert aux tâches 14 et 15. | Trois copies atteignent le seuil de DRY de la maison. | Nul. |
| R5 | La tâche 3 réécrit les trois notes de conformité : la PMP est mémorisée sur le cœur du C3 et appliquée à partir de SP1b-3. | Une documentation connue fausse est une dette. | Nul. |
| R6 | `docs/registers-accepted-without-effect.md` est réécrit par le test qui l'épingle quand on lance `INKEMU_BLESS=1 cargo test -p soc-esp32c3 --test accepted_without_effect` ; sans cette variable, le test échoue si le fichier diffère. | Recopier le tableau à la main dans six tâches. | Un test qui écrit un fichier de l'arbre source, uniquement sur demande explicite. |
| R7 | Les opérations refusées par SPI1, par l'accélérateur SHA et par `RTC_CNTL` sont gardées dans une file (`VecDeque`) que le bus vide après chaque accès, et non dans un `Option`. | Une écriture de `CMD` avec plusieurs bits refusés ne gardait que le dernier refus : un opcode refusé pouvait disparaître (correction de la tâche 5, reprise aux tâches 8 et 12). | Nul. |
| R8 | `inkemu rom fetch` signale une ELF voulue mais absente de l'archive avant d'en stocker une autre (tâche 15). | L'extraction se fait en une seule passe : l'absence est connue avant toute écriture. | Nul. |
| R9 | Aucun registre n'a été ajouté par la tâche 16 : la chaîne complète a passé du premier coup. | Les registres de l'application jusqu'à sa première ligne (BA §3) étaient déjà modélisés (D29). | Nul. |

Les décisions de conception D1 à D29 du plan restent valables, sauf D20 remplacée par R1. Celles que les plans suivants doivent connaître : la flash est en lecture seule (D3) ; les fenêtres de cache sont une vue en direct de la flash (D4, ADR 0002) ; les watchdogs n'expirent jamais (D11) ; les registres PMP sont mémorisés mais pas appliqués (D18) ; le CPU tourne à 20 MHz au reset (D21, une hypothèse).

## Reporté au plan SP1b-3 (interruptions, timers, watchdogs, reset)

- **Premiers registres à modéliser** : `TIMG0.RTCCALICFG`, `RTCCALICFG1`, `RTCCALICFG2` (la calibration doit se dire prête, avec un résultat plausible : `esp32c3-bootloader-accesses.md`, §9) ; puis `SYSTIMER.UNIT0_OP` (bit de valeur valide), le timer RTC (`TIME_UPDATE`, `TIME_LOW0/HIGH0`), puis l'interruption logicielle `SYSTEM.CPU_INTR_FROM_CPU_0` qui lance l'ordonnanceur (même rapport, §9).
- **Écritures en flash** : programmation de page, effacement de secteur et de bloc, écriture du registre d'état par SPI1 (`FLASH_PP`, `FLASH_SE`, le chemin `USR`), lectures de données par SPI1 (`0x03`, `0x0B`, `0xBB` : `esp_flash` lit ainsi), fichier de flash par appareil qui survit aux redémarrages (spec §5.1). Décider alors si les fenêtres de cache restent une vue en direct ou gardent des lignes périmées jusqu'à leur invalidation (ADR 0002).
- **`--firmware`** : l'option n'existe pas encore. Composer une flash à partir d'un `.bin` d'application, d'un bootloader et d'une table de partitions du profil (D1) ; tables de partitions dans les profils.
- **Reset logiciel** : les bits 31 et 5 de `RTC_CNTL.OPTIONS0` sont refusés aujourd'hui (D27) ; causes de reset.
- **Le reset doit remettre à zéro l'état que SP1b-2 a ajouté** :
  - la **PMP** : le bit de verrouillage d'une entrée ne peut aujourd'hui jamais être effacé, puisqu'aucun reset de hart ni de machine n'existe ; un reset de hart ou de machine doit effacer l'état PMP (`pmpcfg0..3`, `pmpaddr0..15`). De plus, `pmpcfg` est mémorisé tel quel : les bits réservés ne sont pas masqués (`crates/cpu-rv32/src/csr_esp32c3.rs`) ;
  - les **files d'opérations refusées** de SPI1, de l'accélérateur SHA et de `RTC_CNTL` (R7) : les périphériques gardent les opérations refusées dans une file que le bus vide après chaque accès ; un reset doit vider ces files.
- **Watchdogs** : RWDT, MWDT0/1 et super watchdog sont du stockage (D11). Leur expiration demande d'abord le tick du SYSTIMER. Le critère 4 de la spec (§10.3 : un firmware bloqué est réinitialisé par son watchdog) et la boucle de reset d'une flash vierge ou corrompue (`rst:0x7`, `esp32c3-rom-flash-boot.md`, §H) en dépendent.
- **Application de la PMP** (D18) et déclencheurs de débogage ; le test de la CLI qui compte sur l'absence de `pmpaddr` dans le cœur générique tient toujours.
- **Interruptions d'erreur du cache** : `CACHE_ILG_INT_ST` et `CORE0_ACS_CACHE_INT_ST` sont verrouillés, mais aucune interruption n'est levée (sources 36 et 61).
- **Registres de programmation des eFuse** (`EFUSE.CMD`, `PGM_*`) : non modélisés ; aucun chemin mesuré n'y touche.
- Tout ce que SP1b-1 avait déjà attribué à SP1b-3 (temps des périphériques, temps d'émission et réception de l'UART, livraison des interruptions, journal d'exception avec PC et pile) est inchangé.
- Chaque ligne de `docs/registers-accepted-without-effect.md` dont la dernière colonne est SP1b-3.

## Reporté au plan SP1b-4 (lockstep QEMU, CoreMark, rejeu, hygiène CI)

Hypothèses de ce plan, à trancher ou à masquer dans le lockstep. Le code compte 25 marqueurs `ASSUMPTION:` (`grep -rn "ASSUMPTION:" crates/`) : 18 viennent de ce plan et 7 de SP1b-1 (voir plus bas). Les lignes 1 à 14 et 16 à 18 du tableau correspondent chacune à un marqueur de ce plan (la ligne 18 en regroupe deux, un par profil). La ligne 15 est un comportement qui n'a pas de marqueur ; les lignes 19 à 21 sont des écarts connus avec l'oracle QEMU qui n'en ont pas non plus.

| # | Hypothèse | Où | Ce qui la tranche, et écart avec l'oracle QEMU |
|---|---|---|---|
| 1 | Un accès à travers une entrée MMU invalide, ou vers une page au-delà de la fin de la flash, renvoie 0 | `bus.rs`, `flash_read` | QEMU renvoie 0 pour une entrée jamais projetée et `0xDEADBEEF` après un passage de valide à invalide ; le lockstep, puis un appareil (`esp32c3-cache-mmu.md`, §6.2 et §9 point 1) |
| 2 | Une écriture dans une fenêtre de flash est une faute d'accès en écriture | `bus.rs`, `write` | identique à QEMU (`mcause` 7) ; le silicium verrouille l'état mais la faute n'est pas établie (même rapport, §9 point 4) |
| 3 | `EXTMEM.ICACHE_SYNC_CTRL` se lit « terminé » (`0x2`) dès le reset | `periph/extmem.rs`, `SYNC_DONE` | QEMU lit 0 au reset (`esp32c3-rom-flash-boot.md`, §I point 4) ; la lecture du registre juste après le reset d'une puce réelle |
| 4 | Une entrée MMU fait 9 bits | `periph/mmu.rs`, `ENTRY_BITS` | identique à QEMU (`0xFFFFFFFF` se relit `0x1FF`) ; le silicium garde peut-être le bit 9 (`esp32c3-cache-mmu.md`, §9 point 6) |
| 5 | Toutes les entrées MMU sont invalides à la mise sous tension | `periph/mmu.rs`, `MmuTable::new` | pas de mesure : la ROM réécrit les 128 entrées avant d'en utiliser une (même rapport, §9 point 7) |
| 6 | Le CPU tourne à 20 MHz au reset (quartz ÷ 2) | `clock.rs`, `cpu_frequency` | QEMU initialise `SYSCLK_CONF` sur la PLL : le bootloader y saute la mise en route de la PLL (`esp32c3-bootloader-accesses.md`, §5 étape 8) ; un journal horodaté d'un appareil, ou le banc du sous-projet 8 (même rapport, §10 point 1) |
| 7 | `ANA_CONFIG`, `ANA_CONFIG2` et `I2C_MST_ANA_CONF0` valent 0 au reset | `periph/ana_i2c.rs`, champ `config` | QEMU répond `0xFFFFFF` à toute lecture de la fenêtre regi2c ; les valeurs de reset réelles sont inconnues, le lockstep doit masquer ces lectures |
| 8 | Un registre analogique regi2c jamais écrit se lit 0 | `periph/ana_i2c.rs`, lecture | idem ligne 7 ; tous les usages mesurés sont des lecture-modification-écriture |
| 9 | Une lecture SFDP répond `0xFF` | `flash.rs`, `NorFlash::execute` | la flash de QEMU rejette `0x5A` (`Unknown cmd 5a`) ; la table SFDP de la vraie puce, lue sur un appareil |
| 10 | `W0[31:24]` se lit 0 après une lecture d'identifiant | `periph/spi_mem.rs`, `run_hard_wired` | non comparé ; la ROM masque cet octet (`esp32c3-rom-flash-boot.md`, §L point 7) |
| 11 | RDSR ne remplace que `RD_STATUS[7:0]` | `periph/spi_mem.rs`, `run_hard_wired` | non comparé (même source) |
| 12 | Tout transfert `USR` a une phase de commande de 8 bits dont l'opcode est l'octet bas de `USER2` ; le bit 31 de `USER` (phase de commande présente) et le champ de longueur de commande (`USER2[31:28]`) ne sont pas lus | `periph/spi_mem.rs`, `run_user` | tous les transferts mesurés ont `USER2[31:28]` = 7 (`esp32c3-rom-flash-boot.md`, §B.2) ; un firmware qui émet une commande sur 16 bits ou sans commande serait exécuté comme une commande de 8 bits, sans diagnostic. Le lockstep, ou une garde qui refuse ces cas par un `Unsupported` |
| 13 | L'adresse d'un transfert `USR` est le champ bas de `ADDR`, sans décalage | `periph/spi_mem.rs`, `run_user` | seule la lecture SFDP utilise une adresse sur les chemins mesurés, et sa réponse n'en dépend pas ; le lockstep |
| 14 | Les octets non lus du dernier mot de données (`W0..`) se lisent 0 | `periph/spi_mem.rs`, `store_data_in` | non comparé (`esp32c3-rom-flash-boot.md`, §L point 7) |
| 15 | **Sans marqueur** : la longueur de données d'un transfert `USR` (le champ de `MOSI_DLEN` ou de `MISO_DLEN`, plus un, en bits) est arrondie à l'octet supérieur et plafonnée à 64 octets | `periph/spi_mem.rs`, `data_len` | rien ne vérifie le comportement du silicium pour une longueur qui n'est pas un multiple de 8 bits ; le lockstep, et un marqueur `ASSUMPTION:` à poser avec la garde de la ligne 12 |
| 16 | `SHA.MODE` 0 est SHA-1 et 1 est SHA-224 (2, SHA-256, est mesuré) | `periph/sha.rs`, `Algorithm` | non exercé par le démarrage ; l'énumération `SHA_TYPE` de `esp32c3/rom/sha.h` d'ESP-IDF, lisible dans l'image Docker de référence |
| 17 | Le contenu des eFuse est celui d'une puce de production v0.3 (bloc v1.2, échantillon de la documentation d'Espressif), pas celui du X4 | `periph/efuse.rs`, `PRODUCTION_V0_3` | QEMU : bloc v1.3, ni MAC ni calibration, donc les lectures d'eFuse et la branche de calibration de `rtc_init` diffèrent ; un `espefuse.py summary` d'un appareil |
| 18 | L'identifiant JEDEC de la flash est `0x9D6018` (D2) | `crates/boards/profiles/x4.toml` et `x3.toml` | identique à QEMU, par choix : le bootloader prend la même branche que dans les traces de référence ; un `esptool.py flash_id` d'un appareil |
| 19 | Le générateur aléatoire est une suite à graine fixe (D8) | `periph/apb_ctrl.rs` | QEMU utilise l'aléa de l'hôte, et les valeurs entrent dans des registres et de la RAM dès l'instruction ≈ 635 927 : injecter les valeurs ou masquer (`esp32c3-bootloader-accesses.md`, §10 point 5) |
| 20 | `SPI0.CTRL` est mémorisé, donc le bootloader affiche `SPI Mode : DIO` | `periph/spi_mem.rs` | SPI0 n'est pas un périphérique dans QEMU, qui affiche `SLOW READ` (`esp32c3-rom-flash-boot.md`, §I point 1) : la ligne `SPI Mode` du journal est à masquer |
| 21 | La ROM écrit chaque caractère de sa console sur USB-Serial-JTAG en plus de l'UART (SP1b-1) | `periph/usb_serial_jtag.rs` | dans QEMU, `EP1_CONF` se lit 0 : la ROM abandonne l'écriture sur USB après environ 5 000 lectures (`esp32c3-rom-flash-boot.md`, §I point 8) |

Les 7 marqueurs hérités de SP1b-1 restent ceux de la note `2026-10-11-sp1b-1-carryover.md` : bits conservés par `ustatus`, position du bit d'événement « instructions » du compteur de performance, compteur avec plusieurs événements sélectionnés, fetch à travers une adresse du bus de données, coût d'un accès non aligné, et la révision de silicium des deux profils (`v0.3`).

Autres points pour SP1b-4 :

- **Temps du cache de flash** : `Bus::fetch_stall` renvoie 0 et un défaut de cache ne coûte rien (D6, ADR 0002) ; la simulation du cache (spec §4.2) et la tolérance CoreMark se décident ici, la calibration au sous-projet 8.
- **La mise en route de la PLL, propre au matériel** (`esp32c3-bootloader-accesses.md`, §5, étapes 9 et 10) et la branche de calibration de `rtc_init` s'exécutent dans inkemu et ne peuvent pas être comparées à QEMU.
- **Le chemin de démarrage depuis la flash des ROM rev0 et rev101** n'a pas été exécuté (`esp32c3-rom-flash-boot.md`, §L, point 9) : seule rev3 est testée depuis la flash.
- **Fermeture des bus** des fenêtres de flash : mémorisée, pas appliquée (D5).
- **CI** : le job `reference-boot` récupère `espressif/idf` par étiquette, pas par empreinte ; les empreintes épinglées des fichiers produits le font échouer fermé. Épingler l'empreinte de l'image avec les autres épinglages. Le job n'a pas encore tourné sur GitHub.
- Tout ce que SP1b-1 avait déjà attribué à SP1b-4 est inchangé.

## À prévoir plus tard

- **SP2** : `--device <fichier.toml>` et le nettoyage du message d'erreur de `toml` ; broches GPIO, matrice GPIO et `IO_MUX` ; le SAR ADC ; chaque ligne de la table des registres acceptés dont la dernière colonne est SP2.
- **SP5a** : registres d'alimentation des mémoires de la radio. **Avant le Wi-Fi par le réseau de l'hôte**, l'adresse MAC par défaut (`58:cf:79:0f:96:8c`) est celle de la puce échantillon de la documentation eFuse d'Espressif : elle est partagée par tous les appareils émulés et elle est universellement administrée. Elle doit devenir une valeur par profil, localement administrée.
- **SP8** : durées des transferts SPI et du SHA (`esp32c3-rom-flash-boot.md`, §L, point 1) ; fréquence du CPU au reset (hypothèse 6).
- La puce de flash, le contenu des eFuse et la révision de silicium d'un Xteink X4 réel restent inconnus : `esptool.py flash_id`, `espefuse.py summary` et le journal de démarrage d'un appareil trancheraient les trois.
