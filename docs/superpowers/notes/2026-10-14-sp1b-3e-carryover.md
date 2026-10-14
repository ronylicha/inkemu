# SP1b-3e — deux firmwares X4 réels démarrent — note de passation

Date : 2026-10-14. Spec : `docs/superpowers/specs/2026-10-14-sp1b-3e-design.md` (décisions E1 à E10). Plan : `docs/superpowers/plans/2026-10-14-sp1b-3e-real-firmwares.md`. Recherche : `docs/research/spi1-fsm-and-usj-reset.md`. Branche `sp1b-3e-real-firmwares`.

## Résultat

- **Console par défaut fixée par le profil** (E1, E2, E9). Chaque profil porte une clé obligatoire `console` (`uart0`, `uart1` ou `usb-serial-jtag`), validée au chargement : absente, le profil est refusé avec les trois valeurs admises (`ProfileError::MissingConsole`) ; inconnue, refusée à sa ligne avec les mêmes valeurs. Le X3 et le X4 nomment `usb-serial-jtag`, où CrossPoint écrit son journal (port USB-C du X4 ; pour le X3, une hypothèse : son câble magnétique porte-t-il les données USB ?). Sans `--console`, `inkemu run` montre la console du profil ; la première ligne de l'erreur standard dit laquelle et d'où vient le choix : `console: usb-serial-jtag (the default of the device profile; --console shows another)` ou `console: uart0 (--console)`, entre le modèle de temps et la flash. Les tests et les exemples du README qui lisent UART0 passent `--console uart0` ; la console affichée n'entre pas dans la trace, aucune empreinte ne bouge.
- **SPI1 `+0x54`** (E3) est `SPI_MEM_FSM_REG`, le registre que l'en-tête d'ESP-IDF définit pour les deux contrôleurs et que le SVD ne liste que pour SPI0. Lu par SPI1, il vaut `0x200` au reset : les deux machines d'état au repos (un transfert finit dans l'écriture de `CMD` qui le lance), le délai de verrouillage 4 en [11:7], seul champ qu'une écriture change. Plus d'accès « non documenté » pour le pilote de flash d'ESP-IDF 4.4, qui interroge ce registre avant chaque lecture d'état (recherche §A).
- **`Rebooting...` absent de l'USB-Serial-JTAG après un reset du cœur** (E4). Verdict de la recherche (§B) : indécidable sans matériel. Le modèle est gardé ; son `ASSUMPTION:` cite maintenant le TRM (§6.1.3 et §30.3.4), le gestionnaire de panique d'ESP-IDF et `esp_restart_noos_dig`, et nomme l'expérience qui tranche.
- **Tests locaux des deux firmwares** (E5 à E7), `crates/emu-cli/tests/real_firmware.rs`, ignorés, hors CI : CrossPoint 1.6.5 (`INKEMU_CROSSPOINT_X3_X4`, `--firmware`) et le firmware d'origine du X4 (`INKEMU_STOCK_X4_FLASH`, `--flash`). Chacun vérifie le SHA-256 épinglé avant toute exécution, lit la console du profil, les avertissements nommés, l'absence d'accès non documenté, puis refait l'exécution en `--strict-mmio` : code 3, registre nommé, et le diagnostic de l'arrêt égal au premier avertissement de l'exécution non stricte.
- **Preuve en CI** (E8) : `tools/reference-firmware/adc_wait_panic/`, à nous (Apache-2.0), construit par `build.sh` comme `usb_serial_jtag_console`. Il lit `I2C0.SR`, lance une conversion du SAR ADC par ses registres (canal 1, atténuation 3, comme CrossPoint), attend `APB_SARADC.INT_RAW` bit 31 sous `portENTER_CRITICAL` (comme `adc_oneshot_read`), le watchdog d'interruption panique, la panique écrit un core dump dans la partition `coredump` (table du X4, `CONFIG_ESP_COREDUMP_ENABLE_TO_FLASH`), et la seconde vie dit qu'elle l'a trouvé. Test `x4_runs_the_adc_wait_panic_reference_firmware_through_its_panic_into_a_second_life` (`run_cli`, job `reference-boot`, effectif 9 → 10).
- **Documents** (E10) : README (section « Real firmwares of the X4 », état, feuille de route, `--console`, aide), README de `tools/reference-firmware`, corrections ajoutées en fin de la note SP1b-3c et de la recherche FW (§K).

## Mesures du 2026-10-14 (binaire release, `--max-time 5`, console du profil)

Relevées avant la branche, sur la machine de l'utilisateur ; les tests de `real_firmware.rs` les tiennent.

- **CrossPoint 1.6.5** (`8eccf614…0385764`, console `usb-serial-jtag`) : première vie `[209] [INF] [HW] Xteink probe scores: pass1=0 pass2=0 verdict=0`, `[209] [XTDET] NVS hw_calib/screenType: not set`, `[277] [XTDET] bus probe VER=00 00 00 00 00 FLG=00 -> default controller`, puis la panique `Guru Meditation Error: Core  0 panic'ed (Interrupt wdt timeout on CPU0).` et `rst:0xc (RTC_SW_CPU_RST)` ; seconde vie `[5] [INF] [HW] Using cached device type: X4`, `[74] [ERR] [GYR] SDK IMU not found`, `[74] [INF] [CLK] RTC not found`, `[74] [INF] [MAIN] Hardware detect: X4`, `[2075] [SD] SD card not detected (err=0x01 data=0x00 cs=12 sclk=8 miso=7 mosi=10 clk=40000000Hz)`, `[2075] [ERR] [MAIN] SD card initialization failed`, `[2115]   Wait complete:  CMD_SOFT_RESET (0 ms)`, puis la même panique. (Une version antérieure de cette ligne plaçait `Hardware detect: X4` dans la première vie : il n'apparaît qu'après le reset.) Avertissements nommés GPIO, I2C0, SPI2, APB_SARADC ; aucun accès non documenté. `--strict-mmio` : arrêt sur `APB_SARADC.APB_TSENS_CTRL`.
- **Firmware d'origine 3.0.2 EN** (`03866a1f…989a88`, image de 16 Mio) : lignes de la ROM, paniques répétées, `rst:0x3 (RTC_SW_SYS_RST)` ; avertissements nommés GPIO et APB_SARADC ; avant cette branche, 738 lectures de `0x60002054` rapportées « non documentées ». `--strict-mmio` : arrêt sur `GPIO.PIN13`.

**Exécution locale des tests, 2026-10-14, commit `ca362ec`** (`cargo test --release -p emu-cli --test real_firmware -- --ignored --test-threads=1`, sur la machine de l'utilisateur) : les deux tests passent, 368 s au total (mode tolérant puis strict pour chacun). La CI du même commit (exécution 38499945847) est verte sur ses cinq jobs : suites ignorées 19, 28, 13, 10 et 5 ; `check` 1 252 tests passés, 78 ignorés ; couverture 98,30 % des lignes.

## Valeurs que seule la CI produit

- `tools/reference-firmware/SHA256SUMS.inputs-adc-wait-panic` : la première construction a imprimé les trois empreintes sous `unpinned SHA256SUMS.inputs-adc-wait-panic:` et échoué avant de livrer quoi que ce soit (README de `tools/reference-firmware`, « Adding a firmware ») ; elles sont recopiées de son journal (commit `40785e8`). La construction suivante, la seconde, doit donner les mêmes.
- `ADC_WAIT_PANIC_TRACE` (`crates/emu-cli/tests/run_cli.rs`) vaut `trace-hash b217bc3b287080ae`, mesurée par l'exécution 38498719362 de la CI (2026-10-14) : toutes les autres assertions du test y passaient, celle-ci a imprimé l'empreinte de l'exécution.
- Défaut cosmétique connu de `adc_wait_panic_main.c`, laissé tel quel pour garder la construction épinglée (changer la source change l'image, ses empreintes et la trace) : si `esp_core_dump_image_check()` rend `ESP_OK` mais `esp_core_dump_image_get()` échoue, la ligne imprimée est `no core dump in flash (ESP_OK)`, l'erreur de `image_get` n'y figurant pas.
- Le test de `adc_wait_panic` lit la valeur `0x0000c000` de `I2C0.SR` : la valeur de reset documentée que le modèle rend pour un registre qu'il n'a pas (SVD). Le délai de 10 s de temps d'appareil laisse la panique (300 ms d'attente), l'écriture du core dump et la seconde vie ; à resserrer si la CI le montre large.

## Hypothèses (`ASSUMPTION:`)

106 lignes dans les fichiers `.rs` au départ, 107 à la fin : une ajoutée, une réécrite.

- Ajoutée : SPI1 `+0x54` lit au repos ce que lit SPI0 `FSM`, et son délai de verrouillage est gardé à part de celui de SPI0 (un alias, pas un registre partagé) ; ni la valeur lue par SPI1 sur une puce ni le partage du champ entre les deux contrôleurs ne sont mesurés (une lecture de `0x60002054` sur une puce au repos tranche la première : `0x200` attendu ; une écriture de `9 << 7` à `0x60002054` suivie d'une lecture de `0x60003054` tranche le second).
- Ajoutée hors `.rs`, dans `crates/boards/profiles/x3.toml` : les broches pogo magnétiques du X3 portent-elles D+ et D− jusqu'à la puce ? Inconnu (`docs/research/xteink-hardware.md` §7, point 3) ; la valeur `usb-serial-jtag` repose sur le firmware. Un hôte qui énumère `303A:1001` par le câble du X3 tranche.
- Réécrite (E4) : un reset du cœur remet l'USB-Serial-JTAG à zéro, FIFO comprises ; un reset du CPU le garde. Elle cite maintenant TRM §30.3.4 (« not affected by SRST »), qui penche dans l'autre sens, et l'expérience qui tranche : capturer l'USB-Serial-JTAG à travers une panique d'une image 4.4 sur carte, ou une image qui écrit `abc` dans `EP1` sans `WR_DONE`, pose `RTC_CNTL_SW_SYS_RST` et lit ce que l'hôte a reçu : `abc` juste avant `ESP-ROM:` (les octets retenus partent avec la bannière, validés par son premier `\n`) dit que le bloc survit au reset du cœur ; la bannière seule, qu'il a été remis à zéro.

## Décisions prises pendant l'écriture du plan

Voir le tableau du plan (F1 à F8). En bref :
- la ligne de départ nomme la console entre le modèle de temps et la flash, la note de flash restant en fin de ligne (des tests la lisent avec son retour à la ligne) ;
- la clé `console` du profil est lue comme le reste du TOML, à sa ligne ; son absence est une erreur à part entière, comme celle de `[flash]` ;
- les exemples vérifiés du README (`hello_world`, NVS) passent `--console uart0` ; ceux de « Build & try » et de `--firmware` montrent le défaut ;
- `real_firmware.rs` exige `INKEMU_ROM_ELF_DIR`, comme les autres tests de ROM, et ne télécharge rien ;
- l'arrêt strict de chaque firmware est comparé au premier avertissement de son exécution non stricte, ce qui tient « le premier accès non modélisé » sans dépendre d'une lecture humaine du journal ;
- `adc_wait_panic` est lancé avec `--console uart0` (sa console ESP-IDF est UART0, l'USB-Serial-JTAG en console secondaire) ;
- la table de partitions de `adc_wait_panic` est une copie de `tests/fixtures/partitions/x4-stock.csv` dans le projet : le conteneur de construction copie le projet seul.

## Reporté, par héritier

- **SP1b-3f (sommeil profond)** : rien de nouveau.
- **SP2 (interface et X4 visuel)** : le SAR ADC (`INT_RAW` bit 31 après un départ, `SAR1DATA_STATUS` à la valeur de l'échelle de boutons), I2C0 (un maître qui reçoit des NACK, ce que voit un vrai X4), SPI2 et la carte SD, les GPIO, le SSD1677 : c'est là que les deux firmwares franchiront l'attente de l'ADC. L'interface montre les deux consoles ; la console du profil est celle qu'elle ouvre d'abord.
- **SP4** : en fin d'exécution, la console USB-Serial-JTAG affichée reçoit le paquet validé et le FIFO IN non validé (`take_written`) même quand l'hôte n'a jamais ouvert le port (`--usb-host absent` ou `attached`) : des octets qu'aucun hôte n'a pris. Le vidage devrait se limiter à ce qu'un hôte ouvert aurait pu prendre ; la première ligne dit déjà que rien n'atteint cette console tant qu'un hôte n'ouvre pas le port.
- **SP8 (matériel)** : l'expérience de E4 (USB-Serial-JTAG à travers un reset du cœur) ; la lecture de `0x60002054` sur une puce, et si un délai de verrouillage écrit par SPI1 se relit par SPI0 (`0x60003054`).
- **Recherche à corriger** (les fichiers sont des documents datés) : `docs/research/esp32c3-boot-chain.md:201` dit `spimem_flash_ll_host_idle` = `dev->cmd.val == 0`, c'est `spimem_flash_ll_cmd_is_done` (recherche §A.2) ; l'hypothèse 12 de `docs/research/usb-serial-jtag-host.md` se lit avec §B.2 de la nouvelle recherche.

## Limites de preuve

- Rien de ce qui touche aux deux firmwares réels n'a tourné dans la CI, ni sur l'hôte de l'architecte : les lignes attendues sont celles des mesures du 2026-10-14 ci-dessus. Les tests de `real_firmware.rs` sont à lancer sur la machine de l'utilisateur avant la fusion.
- `adc_wait_panic` n'a été ni construit ni exécuté par l'architecte (ni Docker ni ESP-IDF sur son hôte) : écrit contre les en-têtes d'ESP-IDF v5.5.2 (`apb_saradc_reg.h`, `i2c_reg.h`, `soc.h`, `esp_core_dump.h`, `Kconfig` d'`espcoredump`, `adc_oneshot.c`), il est vérifié par la première exécution de la CI.
- SPI1 `FSM` est tenu au niveau du périphérique (`spi1_fsm_reads_idle_and_keeps_only_its_lock_delay`) et de la machine en `--strict-mmio` (`spi1_fsm_is_read_idle_without_a_diagnostic_under_strict_mmio`) ; que les 738 lectures du firmware d'origine passent sans avertissement n'est tenu que par son test local.
