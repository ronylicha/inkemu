# SP1b-3d — consoles série dans les deux sens — note de passation

Date : 2026-10-13. Plan : `docs/superpowers/plans/2026-10-13-sp1b-3d-serial-consoles.md`. ADR 0006 (`docs/adr/0006-entrees-des-consoles.md`). Pull request #6, branche `sp1b-3d-serial-consoles`.

## Résultat

- Un firmware lit sa console sur UART0, UART1 et USB-Serial-JTAG : réception UART (FIFO de 128 octets, débordement signalé), interruptions UART (sources 21 et 22) et USB-Serial-JTAG (source 26), durée d'une trame selon `CONF0` dans les deux sens, hôte USB `absent`, `attached` ou `open` avec SOF à chaque milliseconde. Les modes que le modèle n'a pas sont refusés par leur nom : bouclage, ligne de réception inversée, détection du débit, absence de bit de stop, RS485, FIFO d'une autre taille, et depuis l'exécution contrôle de flux (`TX_FLOW_EN`, `RX_FLOW_EN`), ligne d'émission inversée, trame infrarouge et break.
- Les entrées d'une exécution comptent l'hôte USB et les octets envoyés aux consoles, datés en temps d'appareil (ADR 0006). Un script de console (`--console-script`, crate `console-script`) les fournit ; un `wait-for` date les étapes suivantes de l'instant exact où son texte a atteint l'hôte, quel que soit le découpage de l'exécution.
- `inkemu run` gagne `--console-script`, `--usb-host` (`open` par défaut), les lignes de résumé `inkemu: USB host <état>` et `inkemu: console script …`, et le code de sortie 5.
- Jalons tenus dans la CI (`crates/console-script/tests/reference_firmware.rs`) : `uart_select` (délai d'attente 5 s après `app_main`, entre 4 990 et 5 020 ms, puis `Received: a`, puis `bc\r\n` en une rafale), `usb_serial_jtag_echo` (écho, sept rangées de vidage sur UART0, 90 octets lus en 64 + 26), `usb_serial_jtag_console` (`ready` et écho avec l'hôte ouvert, rien du tout sans hôte), `hello_world` (mêmes octets sur USB-Serial-JTAG que sur UART0), déterminisme sous deux découpages.
- CI verte du 2026-10-13, exécution 38470425145 (PR #6) :
  - job `rom-boot` : `real_rom` 19/19, `run_cli` 28/28 ;
  - job `reference-boot` : `reference_boot` 13/13, `run_cli` `reference_firmware` 9/9, `console-script` `reference_firmware` 5/5 ;
  - job `check` : 1 240 tests passés, 75 ignorés ; l'étape WebAssembly (`cargo check --target wasm32-unknown-unknown`) compile aussi `console-script` ;
  - couverture : 98,28 % des lignes (`cargo llvm-cov`, seuil 80).

## Valeurs remesurées

- **Empreintes de trace** des deux exécutions épinglées du firmware de référence (`crates/emu-cli/tests/run_cli.rs`, relevées dans le job `reference-boot`, test `run_cli`, exécutions 38448609123 puis 38460969196, identiques ; épinglées par a8663b3) :
  - `x4_runs_the_reference_firmware_along_its_pinned_trace` : `10174cc2726c3d77` → `fb9ea628bc25b355`, 11 396 706 µs inchangé, 26 562 395 → 26 559 249 instructions ;
  - `x4_runs_the_reference_firmware_from_its_application_image_along_its_pinned_trace` : `c6b24a1cc226ed4a` → `9a6cf19d47136f33`, 11 403 054 µs inchangé, 26 637 404 → 26 635 331 instructions.

  Cause : la ROM recopie sa console sur l'USB-Serial-JTAG et son extrémité IN attend désormais que l'hôte prenne chaque paquet (50 µs avec un hôte ouvert). Une sonde (PR #7, fermée, branche supprimée) a fait tourner la CI sur 9f7f7b4, la fin des tâches UART : les anciennes empreintes et le test du watchdog y tenaient encore. Le changement vient donc de l'USB-Serial-JTAG (tâches 4 et 5), et aucune dérive des tâches 1 à 3 n'est masquée par le nouvel épinglage. Les notes de SP1b-3b et SP1b-3c citent les anciennes valeurs : ce sont des documents datés.
- **Watchdog sur flash vide** (`crates/soc-esp32c3/tests/real_rom.rs`, `a_blank_flash_is_reset_by_the_main_watchdog_then_by_the_rtc_watchdog`) : les resets prennent maintenant la ROM à `0x4004c0d8` puis `0x4004c0dc`, au lieu de `0x4004c0d4` à chaque fois. Ces adresses appartiennent à la boucle d'attente de la FIFO TX de `uart_tx_one_char` (quatre instructions `lw`/`srli`/`andi`/`bnez`, de `d4` à `dc`) : le temps passé sur l'USB-Serial-JTAG décale la phase dans la boucle. Le test accepte désormais toute la boucle (`ROM_UART_WAIT_LOOP`) et vérifie chaque `Saved PC:` contre le PC du journal de son reset ; `a_device_time_limit_ends_the_run_of_a_blank_chip_after_its_first_watchdog_reset` (`run_cli.rs`) fait de même. La phase exacte reste tenue par les empreintes de trace.
- **Empreintes des trois nouvelles constructions**, relevées dans le job `reference-boot` de l'exécution 38446679869, identiques dans sa relance (seconde construction propre), épinglées par 7ab43fb :
  - `SHA256SUMS.inputs-uart-select` : `879fed8f…` (`uart_select.bin`) ;
  - `SHA256SUMS.inputs-usj-echo` : `55b901c3…` (`usb_serial_jtag_echo.bin`) ;
  - `SHA256SUMS.inputs-usj-console` : `c64fb84a…` (`usb_serial_jtag_console.bin`). Son bootloader, `22afedf8…`, diffère du bootloader livré (`601b4845…`) : la configuration de console USB-Serial-JTAG change aussi le bootloader. Seule l'application sort du conteneur, et les tests la démarrent derrière le bootloader livré.

  La table des partitions des trois est celle de `hello_world` (`7f00b6c0…`).
- **Exemples du README** : trois lignes des résumés du firmware de référence avaient dérivé (PC, cycles, instructions). Elles sont recopiées du journal de l'exécution 38463951882 (330402b, 5fce441), et le test ignoré `the_readme_shows_what_the_binary_prints_for_the_reference_firmware` (job `reference-boot`) les compare désormais à la sortie du binaire.
- **Budget des exécutions scriptées de `run_cli`** : `--max-time` passe de 0,05 à 0,1 s (`SCRIPTED_RUN_SECONDS`). Les lignes de la ROM et l'écho finissent avant 51 ms ; le budget en laisse deux fois plus.
- **Ce qui n'a pas bougé** : la fenêtre de 4 990 à 5 020 ms du jalon 1, les sept rangées de vidage du jalon 2, l'égalité des deux consoles du jalon 4.

## Hypothèses (`ASSUMPTION:`)

87 lignes dans les fichiers `.rs` au départ, 106 à la fin : 20 ajoutées, une remplacée. Le plan en annonçait 101 ; l'exécution en a ajouté trois sur l'UART et deux sur l'USB-Serial-JTAG (voir les décisions plus bas).

- **UART** (`periph/uart.rs`, `diag.rs`) :
  - l'octet qui trouve la FIFO pleine est perdu, les 128 restent (UR A2 ; dans `diag.rs`, `UartRxOverflow`, et dans `land`) ;
  - `STOP_BIT_NUM` = 0 dure un bit de stop (UR A4). Cette ligne remplace l'hypothèse d'avant, « un octet dure 10 bits » ;
  - l'hôte envoie au débit et dans la trame de l'UART (UR §6) ;
  - `RXFIFO_TOUT` demande un octet dans la FIFO et une ligne au repos, une fois par période de repos (UR A3) ;
  - `RXFIFO_FULL` et `TXFIFO_EMPTY` sont des niveaux : une condition qui tient encore après `INT_CLR` relève son bit (UR A1) ;
  - un octet qui arrive horloge de réception coupée ou FIFO en remise à zéro est perdu (UR A8) ;
  - une lecture de la FIFO vide rend 0 (UR A6) ;
  - ajoutées à l'exécution : un octet reçu garde le temps de bit de son bit de start ; un changement du temps de bit pendant une période de repos la reminute depuis l'atterrissage, et un délai d'attente que ce changement place dans le passé vient à l'instant du changement ; le délai d'attente se compte avec l'horloge du récepteur (un octet perdu horloge coupée n'ouvre pas de période de repos, un bit de start horloge coupée n'en ferme pas, aucun délai d'attente n'est levé horloge coupée) ; le récepteur décode une trame depuis son bit de start, si bien qu'un octet dont le bit de start est venu horloge coupée est perdu même si l'horloge revient avant la fin de sa trame (même ligne `ASSUMPTION:` que le délai d'attente ; une coupure de `RX_SCLK_EN` pendant une trame sur une carte la tranche).
- **USB-Serial-JTAG** (`periph/usb_serial_jtag.rs`, `periph/system.rs`) :
  - les registres d'état des extrémités valent leur valeur de reset, personne ne les lit (USJ §A) ;
  - l'hôte prend un paquet 50 µs après sa validation (USJ §D.2, O.5) ;
  - un hôte au port fermé émet des trames et ne prend rien ; un branchement depuis `Absent` lève `USB_BUS_RESET`, un débranchement ne lève rien (USJ O.3) ;
  - un hôte branché à la mise sous tension commence ses trames à 0 sans reset de bus ;
  - un reset du cœur remet le bloc à zéro, un reset du CPU le garde (TRM §6.1.3, IR §4.2) ;
  - `WR_DONE` pendant qu'un paquet attend l'hôte ne fait rien ;
  - une lecture de la FIFO OUT vide rend 0 ; un octet écrit sans place est ignoré (USJ §R.1, O.6) ;
  - ajoutées à l'exécution : un paquet de longueur nulle de l'hôte lève `OUT_EP1_ZERO_PAYLOAD` mais pas `SERIAL_OUT_RECV_PKT` ; l'impulsion de `SYSTEM_USB_DEVICE_RST` (bit 23 de `PERIP_RST_EN0`) remet le bloc à zéro comme un reset du cœur, sans que l'hôte la voie.
- **Autres** : aucune.

## Décisions prises pendant l'écriture du plan

- Le reset du cœur remet l'USB-Serial-JTAG à zéro. Le rapport USJ (O.12) lisait le tableau de IR §4 comme « le bloc est conservé » ; la ligne qui le conserve est celle du reset du CPU (`esp_restart`, `rst:0xc`), et inkemu n'appelle `UsbSerialJtag::reset` qu'au reset du cœur.
- Les valeurs de registres des exécutions QEMU (UR §3.3) sont relues par QEMU, qui rend 0 pour les bits qu'il ne garde pas : le modèle suit les valeurs qu'écrit le pilote ESP-IDF.
- Le code de sortie 5 ne remplace que 0 : un CPU qui attend pour toujours garde 4, un arrêt strict 3.
- Les autres : voir le tableau des décisions du plan (C13 à C30).

## Décisions prises pendant l'exécution

**Conduite.** Les onze commits du plan ont été repris par `cherry-pick` de l'arbre de travail où l'architecte les avait construits et vérifiés (`0fd9335..5ce76ae` → `4843ab1` à `a460dbf`) ; la branche ne différait de cet arbre que par le fichier du plan. Pas de passe préalable : l'architecte avait vérifié mécaniquement que les blocs du plan reconstruisent ses commits. Quatre relectures par domaine (UART ; USB-Serial-JTAG et hôte ; entrées et script ; CLI, tests, firmwares et documents), en lecture seule, puis des correcteurs l'un après l'autre, chacun exécutant les mutations des relectures ; une seconde relecture des correctifs ; la CI comme recette.

**UART.**
- Une UART cadencée par `APB_CLK` se met à jour avant un changement d'horloge, si bien qu'un octet de l'hôte et le délai d'attente de réception arrivent aux mêmes instants quel que soit le découpage (528c87c). Tests : `a_byte_and_a_timeout_on_the_apb_clock_come_at_the_same_instants_whenever_the_uart_is_called`, `a_uart_on_the_apb_clock_raises_its_timeout_at_the_instant_the_new_rate_gives`, `a_byte_on_the_apb_clock_lands_the_same_whatever_the_calls_when_the_clock_changes_under_it`.
- Un octet reçu ou un délai d'attente minuté sur une horloge source que le modèle n'a pas est refusé (`Unsupported::UartReceiveClock`) à la lecture du firmware qui le montre, et non au bit de start : l'instant d'un bit de start dépend du découpage, celui d'une lecture non (43eb585). Un reset oublie ce défaut, sauf pour l'octet encore sur la ligne, et un octet que le récepteur n'entend pas n'est pas refusé (f382f28). Tests : `what_the_receiver_shows_on_a_source_clock_the_model_does_not_have_is_refused_when_the_firmware_reads_it`, `a_reset_forgets_what_the_receiver_showed_on_a_clock_the_model_lacks_and_not_the_byte_on_the_line`, `a_byte_nobody_hears_on_a_clock_the_model_lacks_is_not_refused`.
- Un récepteur dont l'horloge est coupée ne compte aucun délai d'attente ; un octet qu'il n'entend pas n'ouvre ni ne ferme de période de repos (a1aed9a, f382f28). Un octet dont il n'a pas entendu le bit de start n'entre pas dans la FIFO, même si l'horloge revient avant son atterrissage : un récepteur qui a manqué le front de start ne décode pas la trame (d1d96b4). Ce choix, plus fidèle que l'entrée de l'octet qu'épinglait f382f28, rend sans effet l'ordre du délai d'attente et de l'atterrissage au même instant. `rx_mistimed` est marqué par un bit de start ou un atterrissage que le récepteur entend, même si l'octet est perdu ensuite : la fin de la période de repos est visible du firmware. Tests : `a_receiver_whose_clock_is_off_counts_no_timeout`, `a_byte_whose_start_bit_comes_with_the_receiver_clock_off_is_lost_and_ends_no_idle_period`.
- Contrôle de flux, ligne d'émission inversée, trame infrarouge et break sont refusés par leur nom, comme le bouclage (671aeb3). Test : `flow_control_an_inverted_transmit_line_irda_and_a_break_are_refused_by_name`.
- Les octets de l'hôte sont gardés une fois chacun, leurs dates par séries ; une entrée datée au plus tard comme la précédente rejoint sa série (9c581c8). Test : `an_input_dated_no_later_than_the_one_before_joins_its_run`.
- Une date au-delà du dernier instant où une UART date sa ligne (`LAST_INPUT_DATE`, environ 114 ans) est refusée (`InputError::BeyondTheLine`) au lieu de saturer (c60415b). Test : `an_input_dated_after_the_last_instant_a_uart_times_is_refused_and_changes_nothing`.
- Seuils hostiles (0, au-delà de la FIFO), délai d'attente activé pendant une période de repos ; la boucle de `settle` est bornée et un défaut la fait échouer au lieu de geler la suite (bb5463b).

**USB-Serial-JTAG et hôte.**
- À un même instant, un changement d'hôte passe avant la prise d'un paquet, deux changements s'appliquent dans l'ordre donné, et un hôte rebranché recommence ses trames à son branchement (6805570). Tests : `a_host_that_closes_its_port_at_the_instant_it_would_take_a_packet_does_not_take_it`, `two_changes_of_the_host_at_the_same_instant_apply_in_the_order_given`, `a_host_attached_again_starts_its_frames_from_the_new_attach`.
- Un reset du cœur remet `FRAM_NUM` à 0 jusqu'à la trame suivante (4631d25). Test : `a_core_reset_sets_fram_num_to_0_until_the_next_frame`.
- Les octets que l'hôte doit encore envoyer sont comptés à mesure (15d0853) ; un changement d'hôte échu est appliqué avant que les changements à venir soient comptés, et `usb_host_changes_pending` prend `&mut self` (0f45a7c, test `a_change_of_the_host_due_by_now_is_not_counted_as_to_come`) ; aucun réveil n'est demandé à un début de trame tant que `SOF` est levé, et le compte des trames ne déborde pas (24c22e1, tests `a_raised_sof_asks_for_no_call_until_it_is_cleared`, `frames_counted_up_to_the_end_of_device_time_do_not_overflow`).
- Une écriture d'`INT_RAW` est refusée, comme pour l'UART et TIMG, plutôt que de suivre USJ §R.1 à la lettre (adafba5).
- Le bit 23 de `PERIP_RST_EN0` (`SYSTEM_USB_DEVICE_RST`, vérifié dans `system_reg.h`) remet l'USB-Serial-JTAG à zéro, l'hôte restant branché (2ec3168). Test : `a_reset_of_the_usb_device_empties_usb_serial_jtag_and_keeps_its_host`.
- Nouveau test ignoré : la vraie ROM, sans hôte ou port fermé, garde son premier paquet USB et imprime sa bannière sur UART0 (df8033c, `rev3_rom_keeps_its_first_usb_packet_without_a_host_that_takes_it_and_goes_on`) ; `real_rom` passe de 18 à 19 dans la CI.
- La Review Focus du plan nomme mal un test et sa mutation : le test s'appelle `a_core_reset_empties_the_block_and_the_host_goes_on` (sans `_sending`), et la mutation qui le fait échouer ajoute `self.sent.clear();` dans `reset`.

**Entrées de la machine et script.**
- `Frequency::nanos_to_cycles` (arrondi au cycle supérieur, saturant) sert à la fois à `--max-time` et au script (b6f19e7) ; les `wait` qui suivent un `wait-for` sont additionnés exactement et arrondis une seule fois (452a7ba). Test : `the_waits_are_added_up_and_rounded_up_to_a_cycle_of_the_time_base_once`.
- `input_pending` compte, sur chaque canal, les octets de l'hôte qui n'ont pas encore atteint l'appareil ; un envoi UART vide ne compte plus (3e25dba). Test : `the_input_pending_on_every_channel_is_what_has_not_reached_the_device`.
- Un `wait-for` ne cherche jamais d'octets envoyés avant son étape et ne date jamais une étape avant la précédente (452a7ba). Test : `a_wait_for_after_a_wait_ignores_the_rest_of_the_instant_it_follows`, seul à faire échouer la mutation qui retire ce contrôle (`a_wait_for_on_the_same_instant_with_no_wait_between_reads_the_rest_of_it` tient `<` → `<=`).
- Un canal cesse d'être observé dès qu'aucun `wait-for` à venir ne le nomme ; `ScriptRun::stop` libère les canaux, et emu-cli l'appelle quand la machine refuse une étape (452a7ba) ; `ScriptRun::start` l'appelle aussi avant de rendre son erreur (208a05d). Tests : `a_channel_no_wait_for_to_come_names_is_no_longer_watched`, `a_step_the_machine_refuses_ends_the_script_and_its_channels_go_back_to_their_pace`, `a_script_refused_at_its_start_leaves_no_channel_watched`.
- Analyseur : les messages citent le script avec ses caractères de contrôle échappés et coupé à 40 caractères ; un BOM et un CR seul sont refusés pour ce qu'ils sont ; la ligne de version accepte des espaces et un commentaire après elle (d596ba0). Tests : `a_message_quotes_the_script_with_its_control_characters_escaped_and_cut`, `a_byte_order_mark_and_a_carriage_return_alone_are_refused_for_what_they_are`, `the_version_line_must_come_first`.
- L'ordre de la machine au même instant (un `usb-host` passe avant un `send usb-serial-jtag`) est écrit dans le format et le README (d596ba0). Test : `a_change_of_the_usb_host_applies_before_a_send_on_usb_serial_jtag_at_its_instant`.
- Un firmware endormi date les étapes de la même façon quel que soit le découpage (7709c37, `a_sleeping_firmware_dates_the_steps_the_same_whatever_the_calls_the_run_is_cut_into`).

**CLI, tests, firmwares et CI.**
- `test_support::drive` : chaque borne est tenue sous `within`, une tranche de 0 cycle est refusée, `take` garde les diagnostics et les écritures de flash de chaque prise (144b8da) ; la garde des exemples de documentation lit aussi les commentaires de bloc et les attributs `doc` (58de109).
- Watchdog sur flash vide : voir « Valeurs remesurées » (a03a71a, 34d4335). Un démarrage de la vraie ROM est aussi borné en temps d'appareil (10 s pour la bannière, 20 s pour un démarrage depuis la flash).
- `--usb-host` est tenu à travers le binaire (34d4335, `the_usb_host_given_is_the_one_the_summary_says`) ; `run_cli` du job `rom-boot` passe à 28. Chaque étape de la CI lance toutes ses suites ignorées et n'échoue qu'à la fin, pour qu'une exécution donne tous les signaux.
- Jalon 3, moitié « hôte absent » : rien sur USB-Serial-JTAG deux secondes après la dernière ligne du bootloader (a7c9952).
- Jalon 2, ce que porte l'USB-Serial-JTAG : le flux USB est celui d'UART0 avec l'écho inséré, et non l'écho seul. C'est la ROM qui recopie, et non la console secondaire d'ESP-IDF, que l'exemple coupe (`CONFIG_ESP_CONSOLE_SECONDARY_NONE`) : l'image est une construction minimale sans VFS (USJ §O.9), dont `printf` et `ESP_LOG` passent par `esp_system_console_put_char` puis par `uart_tx_one_char` de la ROM, qui envoie aussi chaque caractère sur l'USB-Serial-JTAG tant que `g_usb_print` est vrai ; le bootloader livré le met à vrai et l'application ne refait pas la console de la ROM (USJ §B.1, §B.2, §F.1). Le test vérifie aussi que l'écho n'est pas sur UART0, et que les 26 derniers octets sont lus en une fois, par l'adresse de leur seconde rangée (0bc8f16, b2529a7 ; USJ §F.1 en aa11bfc).
- Jalon 2, écart au plan : le script du plan (tâche 10) envoyait les 90 octets dès l'arrivée de l'écho de `hi, espressif\n`. Celui du test attend d'abord la rangée de vidage de cette ligne sur UART0 (`|hi, espressif.|`). Raison : envoyés pendant que la tâche de l'exemple imprime encore ce vidage, les deux paquets atteignaient l'anneau du pilote avant sa lecture suivante, et la tâche lisait les 90 octets d'un coup ; le découpage 64 + 26 que le jalon doit montrer suppose une tâche qui attend dans `usb_serial_jtag_read_bytes`, comme le supposent le test pytest d'Espressif (`sleep(1)` entre deux lignes) et l'exemple tapé à la main de son README. Que la même chose arrive sur une carte reste une hypothèse (USJ §O.13), tranchée par un écho sur carte avec une ligne envoyée à l'arrivée de l'écho précédent (0bc8f16 ; USJ §F.1 en 61d56c5, aa11bfc).
- Chaque test de `reference_firmware.rs` imprime son statut et les deux consoles quand il échoue (0bc8f16).
- Le firmware d'écho des exécutions scriptées de `run_cli` attend une place dans la FIFO TX, que la ROM laisse pleine, avant chaque octet ; un test sans ROM y entre FIFO pleine (cb96fa3, `the_echo_firmware_waits_for_room_in_the_transmit_fifo_the_rom_leaves_full`). La fenêtre de 50 ms n'était pas en cause.
- Les résumés du README disent l'hôte USB (16e74d0) ; un test les compare au binaire (ae6efea) ; `run_cli` `reference_firmware` passe à 9.
- ADR 0006 : relu contre le code final (bornes, ordre au même instant, observation USB par paquet, rejeu dans les bornes) et relié à cette note.
- Cette note porte la date de son écriture, 2026-10-13, et non le 2026-10-14 que le plan prévoyait.

## Reporté, par héritier

- **SP1b-3e (CrossPoint)** : le modèle d'hôte USB sert tel quel au `HWCDC` d'Arduino ; vérifier sur CrossPoint que la première interruption vient de `SERIAL_IN_EMPTY` à sa valeur de reset et que le moniteur voit les SOF.
- **SP2 (interface)** :
  - une console interactive (taper au clavier) avec un temps d'appareil cadencé sur l'horloge murale, par `Esp32C3::send_console` à l'instant présent de la machine ; un interrupteur d'hôte USB (`set_usb_host`) ; l'affichage de la progression d'un script (`ScriptRun::status`) ;
  - le contrat de `watch_console` : prendre le canal avant de l'observer et après chaque appel. `Uart::watch` ne met pas l'UART à jour (`settle`) : sans effet observable, puisque `take_console` le fait déjà, et cela retarderait le signalement des octets partis avant l'observation ;
  - appeler `ScriptRun::stop` quand une exécution s'arrête avant son script ; `usb_host_changes_pending` prend désormais `&mut self` ;
  - un départ refusé (`ScriptRun::start` rend une erreur) laisse dans la machine les entrées des pas qui précèdent le pas refusé, comme le dit sa documentation. La CLI sort aussitôt ; une interface qui afficherait l'erreur et laisserait tourner la machine les recevrait sans script. Elle doit soit écarter cette machine, soit faire valider les dates de tous les pas d'avant le premier `wait-for` avant d'en remettre aucun ;
  - les règles de `flash_state.rs` restent à déplacer (note de SP1b-3c).
- **SP1b-4 (enregistrement et rejeu)** : porter l'état de l'hôte à la mise sous tension et la liste datée des entrées des consoles et des changements d'hôte, telle que la machine les a reçues (ADR 0006), remise dans les bornes de la machine ; la liste blanche du lockstep contre QEMU (UR §10 : QEMU livre les octets d'un coup, sans débordement, et lève un `RXFIFO_TOUT` sur FIFO vide).
- **SP4 (scénarios)** : les scénarios YAML remplacent le format `inkemu-console-script 1`, qu'ils désignent par sa ligne de version.
- **SP8 (temps et matériel)** : un hôte UART à un autre débit (octets corrompus, erreurs de trame) ; la latence de l'hôte USB mesurée sur un vrai hôte ; les hypothèses de cette note qu'une carte trancherait, dont l'écriture d'`INT_RAW` de l'USB-Serial-JTAG (refusée ici) et le commentaire de `usb_serial_jtag_console_main.c` selon lequel les lectures de `stdin` se terminent sans hôte (il suit USJ §F.2 ; le corriger changerait l'empreinte épinglée de l'application).
- **Limites de preuve** :
  - le test du watchdog sur flash vide accepte toute la boucle d'attente de la ROM : un changement qui déplacerait le reset à l'intérieur de cette boucle ne serait vu que par les empreintes de trace ;
  - la garde `observing &&` de `execute_with_script` (emu-cli) est conservée bien que sa mutation soit équivalente : un refus vient toujours d'une étape `send` ou `usb-host`, où aucun `wait-for` n'est en cours ; la garde rend la boucle indépendante de cette propriété interne de `ScriptRun`.
