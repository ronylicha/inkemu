# SP1b-3f — sommeil profond et réveil par le timer RTC — note de passation

Date : 2026-10-14. Spec : `docs/superpowers/specs/2026-10-14-sp1b-3f-design.md` (décisions F1 à F13). Plan : `docs/superpowers/plans/2026-10-14-sp1b-3f-deep-sleep.md` (décisions P1 à P13). Recherche : `docs/research/esp32c3-deep-sleep.md` (parties A et B, « DS » dans le code). Branche `sp1b-3f-deep-sleep`.

## Résultat

- **Découpage (F8)**, dans un commit sans changement de comportement : `periph/rtc_cntl.rs` devient `rtc_cntl/` (`mod.rs`, `timer.rs` pour le timer RTC, `wdt.rs` pour le watchdog RTC, `swd.rs` pour le super watchdog, puis `sleep.rs`), et la calibration de l'horloge lente quitte `timg.rs` pour `timg/calibration.rs`. Les 60 tests de `rtc_cntl` et `timg` passent avant et après, sans changement.
- **Entrée en sommeil profond (F1).** À l'écriture de `STATE0.SLEEP_EN` avec `DIG_PWC.DG_WRAP_PD_EN` à 1, `RTC_CNTL` le signale au contexte et la machine éteint le CPU : plus aucune instruction ; le temps d'appareil saute, en un calcul, jusqu'au prochain appel du domaine RTC (l'alarme du timer RTC, ou son watchdog laissé en marche sans `PAUSE_IN_SLP`), et `RTC_CNTL` est appelé seul : les périphériques numériques n'ont pas d'alimentation. Leurs réveils échus pendant le sommeil sont livrés après le reset du réveil, à des périphériques reconstruits qui n'y trouvent rien à faire (contrat de `Event`). Les deux chemins d'entrée d'ESP-IDF (`rtc_deep_sleep_start`, et `rtc_sleep_start` avec une configuration profonde) passent par la même écriture.
- **Réveil (F2).** Quand le timer RTC atteint la cible 48 bits de `SLP_TIMER0/1` et que le timer est une source de réveil (`WAKEUP_ENA` bit 3), `SLP_WAKEUP_CAUSE` vaut 8, `INT_RAW` reçoit les bits 0 et 10, `SLEEP_EN` retombe et le bloc demande un reset du cœur de cause 5 (`ResetCause::DeepSleep`, `DSLEEP`). Tout ce qu'un reset du cœur garde est gardé (mémoire RTC, timer RTC, `STORE0..7`, registres de `RTC_CNTL`) ; le PC enregistré par `ASSIST_DEBUG` est effacé. Le journal dit `inkemu: woken from deep sleep by the RTC timer (rst:0x5), a core reset; the CPU had stopped at pc …`. Une cible déjà atteinte réveille à l'écriture de `SLEEP_EN`.
- **Unité de CRC de la mémoire RTC rapide (F3)** : `SYSTEM.RTC_FASTMEM_CONFIG`/`RTC_FASTMEM_CRC`. L'écriture qui met `START` à 1 poste un signal que le bus traite (il détient la mémoire RTC) ; `FINISH` vaut 1 dès l'instruction suivante. CRC-32/ISO-HDLC sur les mots `ADDR` à `ADDR + LEN`, bornés aux 2 048 mots de la mémoire.
- **Sommeil léger refusé (F4)** : `Unsupported::LightSleep`, la demande est rejetée (`INT_RAW` bit 1, `SLEEP_EN` à 0), le firmware continue ; arrêt en `--strict-mmio`.
- **Autres sources de réveil (F5)** : `Unsupported::SleepWakeSource`, le GPIO (bit 2) nommé comme fonction du sous-projet 2 ; le sommeil commence quand même, avec le timer pour seul réveil possible. Un sommeil que rien de modélisé ne peut finir rend `StopReason::DeepSleep` (le temps d'appareil n'est pas porté à la limite, comme pour `Idle`), et la CLI finit avec `RunEnd::DeepSleep`, code 0, et la ligne `inkemu: the device is in deep sleep since … us of device time; it waits for …`. Une exécution qui atteint sa limite pendant un sommeil dit quand le timer réveille l'appareil.
- **Table des registres acceptés sans effet (F6)** : plus aucune ligne pour SP1b-3f. `DIG_PWC` sort de la table ; `OPTIONS0`, `PWC`, `SYSTEM.MEM_PD_MASK` vont à SP4, `TIMER1`, `TIMER4`, `TIMER5`, `TIMER6` à SP8, `TIMER3` à SP5a, `DIG_ISO` à SP2 ; entrent les registres du chemin de sommeil qu'aucun modèle ne connaissait : `BIAS_CONF`, `SDIO_CONF` (SP4), `GPIO_WAKEUP`, `EXT_WAKEUP_CONF`, `SLP_REJECT_CONF` (SP2). `PLANS_TO_COME` perd SP1b-3f et gagne SP4.
- **`TIME_UPDATE` bits 29:27 (F7)** : stockés et relus (R/W selon l'en-tête), refusés en les nommant (`Unsupported::RtcTimerTrigger`). Les bits `SW_CPU_INT`, `SLP_WAKEUP`, `SLP_REJECT` de `STATE0` écrits à 1 aussi (`Unsupported::SleepSignal`).
- **Interface (F9)** : `Esp32C3::deep_sleep() -> Option<DeepSleep>` (depuis quand, échéance du timer, sources) ; l'événement de réveil est le `Diagnostic::Reset` de cause 5. La CLI ne fait qu'afficher.
- **Déterminisme (F10)** : `a_deep_sleep_run_is_the_same_whatever_its_calls` passe le même firmware de quatre vies en un appel puis en tranches de 1, 7, 997 et 7 919 cycles : consoles, empreinte de trace, temps d'appareil, cycles et resets identiques.
- **Preuve en CI (F11)** : `tools/reference-firmware/deep_sleep_timer/`, à nous (Apache-2.0), sans `sdkconfig.defaults` propre (la configuration de référence garde `CONFIG_ESP_SYSTEM_ALLOW_RTC_FAST_MEM_AS_HEAP=y`, donc `rtc_deep_sleep_start` et l'unité de CRC). Test `x4_runs_the_deep_sleep_timer_reference_firmware_through_three_wakes_into_a_sleep_without_end` (`run_cli`, job `reference-boot`, effectif 10 → 11).
- **Tests unitaires (F12)** : 13 dans `rtc_cntl/sleep.rs`, 3 dans `system.rs`, 7 dans `machine.rs`, 4 dans `emu-cli/src/run.rs`, plus les listes de `diag.rs` et `reset.rs`.

## Valeurs que seule la CI produit

- `tools/reference-firmware/SHA256SUMS.inputs-deep-sleep-timer` : absent. La première construction imprime les trois empreintes sous `unpinned SHA256SUMS.inputs-deep-sleep-timer:` et échoue avant de livrer quoi que ce soit ; les recopier dans ce fichier, pousser, et vérifier que la construction suivante (propre) donne les mêmes.
- `DEEP_SLEEP_TIMER_TRACE` (`crates/emu-cli/tests/run_cli.rs`) vaut `trace-hash not pinned yet` : le test échoue alors sur cette seule valeur et imprime l'empreinte de l'exécution ; vérifier la console et le journal du message d'échec, puis l'épingler.
- Les millisecondes de sommeil que l'application imprime ne sont pas épinglées : le test les lit dans `2000..3000` (2 s moins les 400 µs qu'ESP-IDF retranche, plus le démarrage de la vie suivante, que fixe le modèle de temps non calibré).

## Exécuté localement (hôte de l'architecte)

`cargo fmt --all` ; `tools/ci/clippy.sh -p soc-esp32c3 -p emu-cli -p console-script --all-targets --all-features -- -D warnings` ; `cargo test -p soc-esp32c3 --lib` (623 passés) ; `cargo test -p soc-esp32c3 --test accepted_without_effect` (2, après `INKEMU_BLESS=1`) ; `cargo test -p emu-cli --lib` (173) ; `cargo test -p emu-cli --test run_cli -- readme help` (7) ; `cargo test -p console-script --lib` (30) ; `cargo test -p emu-cli --test run_cli -- --ignored --list` : 11 tests `reference_firmware`, 28 autres. Rien d'autre : ni suite complète, ni suites ignorées, ni couverture, ni Docker, ni ESP-IDF.

## Hypothèses (`ASSUMPTION:`)

107 lignes dans les fichiers `.rs` au départ, 115 à la fin : huit ajoutées.

- L'alarme du timer RTC part tout de suite quand sa cible est derrière le timer de moins de la moitié de sa course (2^47 coups), comme celle d'un timer de groupe (`timer.rs`). Tranche : activer l'alarme sur une puce avec une cible passée et mesurer le réveil.
- `MAIN_TIMER_ALARM_EN`, en écriture seule, garde la valeur de la dernière écriture de `SLP_TIMER1`, et une alarme partie reste activée (`sleep.rs`). Tranche : un second sommeil profond sans réécrire `SLP_TIMER1`, le timer comme source.
- `SLEEP_EN` se relit à 0 quand la demande est finie, rejetée ou réveillée (`sleep.rs`). Tranche : relire `STATE0` sur une puce après le rejet d'un sommeil léger.
- Un octet qu'une UART envoie encore à l'entrée en sommeil part au réveil, au reset de l'UART, alors que la puce le perd (`sleep.rs`) ; ESP-IDF attend que les UART soient vides (`flush_uarts`). Tranche : les derniers octets avant le sommeil, reçus par un hôte sur une carte.
- L'unité de CRC ne prend pas de temps, compte `ADDR` et `LEN` en mots de 32 bits, couvre `ADDR` à `ADDR + LEN`, et calcule CRC-32/ISO-HDLC (`system.rs`). Tranche : un motif connu en mémoire RTC rapide, l'unité lancée sur quelques mots, le résultat comparé aux candidats.
- `FINISH` vaut 1 de l'écriture qui met `START` jusqu'à celle qui l'efface (`system.rs`). Tranche : lire le registre en boucle autour des deux écritures.
- Un sommeil profond éteint l'assistant de débogage : pas de `Saved PC:` après `rst:0x5` (`periph/mod.rs`). Tranche : le journal de démarrage d'une puce réelle après un réveil.
- La SRAM interne garde ses octets pendant un sommeil profond, alors que la puce les perd (`periph/mod.rs`). Tranche : un mot `.noinit` de SRAM lu après un réveil sur une puce.

## Décisions prises pendant l'écriture du plan

Voir le tableau du plan (P1 à P13). En bref :
- `Event::RtcWatchdog` devient `Event::RtcCntl` : un seul réveil du bloc pour son watchdog, son super watchdog et l'alarme ;
- seul `RTC_CNTL` est appelé pendant un sommeil ; un reset qu'il demande alors (le réveil, son watchdog non mis en pause) termine le sommeil ;
- un sommeil que rien ne peut finir arrête l'exécution sans déplacer le temps, code 0 ;
- l'alarme reste activée jusqu'à la prochaine écriture de `SLP_TIMER1` et lève `MAIN_TIMER_INT_RAW` une fois par cible ;
- l'unité de CRC calcule CRC-32/ISO-HDLC, bornée à la mémoire ;
- les sources autres que le timer sont nommées à l'entrée, le sommeil commence quand même ;
- `SLP_REJECT_CONF` est un stockage accepté (SP2) : aucune source de rejet n'est levée ;
- héritiers des lignes de la table, et SP4 parmi les plans à venir ;
- bits de signal de `STATE0` et bits 29:27 de `TIME_UPDATE` refusés en les nommant ;
- le réveil efface le PC enregistré ; la SRAM est gardée (deux hypothèses) ;
- le watchdog RTC se met en pause en sommeil quand `PAUSE_IN_SLP` est à 1 ;
- `deep_sleep_timer` tourne avec `--console uart0 --max-time 30` derrière le bootloader livré ;
- un octet UART en vol à l'entrée part au réveil (hypothèse).

Le modèle et la CLI ont été commités ensemble (tâches 3 à 5 et 7 du plan) : la nouvelle raison d'arrêt rend le `match` de la CLI non exhaustif, l'espace de travail ne compile qu'avec les deux.

## Reporté, par héritier

- **SP2 (GPIO, interface)** : le réveil par GPIO (`WAKEUP_ENA` bit 2, `GPIO_WAKEUP`, `EXT_WAKEUP_CONF`, maintien des broches `PAD_HOLD`) — c'est ce dont CrossPoint et le firmware d'origine ont besoin pour se réveiller par le bouton (GPIO3) ; le rejet d'un sommeil par une source (`SLP_REJECT_CONF`, `SLP_REJECT_CAUSE`) ; l'isolation des broches (`DIG_ISO`). L'interface montre l'état « sommeil profond » (`Esp32C3::deep_sleep`) et l'écran éteint. L'USB-Serial-JTAG à travers un sommeil : l'hôte devrait voir une déconnexion puis une réénumération (DS question 7) ; aujourd'hui le reset du cœur remet le bloc à zéro et `--usb-host` ne change pas.
- **SP4 (moniteur de ressources)** : la consommation pendant le sommeil ; les forçages d'alimentation de `OPTIONS0`, `PWC`, `BIAS_CONF`, `SDIO_CONF`, `MEM_PD_MASK`.
- **SP5a** : `TIMER3` (minutage d'alimentation de la radio).
- **SP8 (matériel)** : la latence du réveil (`TIMER1`, `TIMER4`, `TIMER6`, et le temps entre l'échéance et la première instruction de la ROM, DS question 2) ; le sommeil minimal (`TIMER5`) ; le polynôme et les unités de l'unité de CRC ; `Saved PC:` après `rst:0x5` ; si la protection de démarrage flash du watchdog RTC est réarmée au réveil (DS question 4) ; la SRAM après un réveil.
- **Hors périmètre, nommé** : le sommeil léger (refusé), les réveils par UART et USB.

## Limites de preuve

- `deep_sleep_timer` n'a été ni construit ni exécuté par l'architecte (ni Docker ni ESP-IDF sur son hôte) : écrit contre les en-têtes d'ESP-IDF v5.5.2 (`esp_sleep.h`, `esp_system.h`, `esp_attr.h`), il est vérifié par la première exécution de la CI. Le chemin réel (ROM au réveil, `rtc_boot_control`, stub de réveil exécuté depuis la mémoire RTC, bootloader livré qui ne recharge pas les segments RTC, démarrage de l'application de cause 5) n'est tenu que par ce test.
- Les registres que la séquence d'ESP-IDF touche ont été relevés dans le code source (DS §A.1) ; un registre oublié apparaîtrait comme avertissement dans ce test, qui n'en tolère aucun.
- Le sommeil est tenu au niveau des registres (`rtc_cntl/sleep.rs`) et de la machine par un firmware écrit en instructions (`machine.rs`), dont le déterminisme est vérifié sur quatre découpages.
- QEMU ne modélise ni le sommeil ni le timer RTC : pas d'oracle au-delà de l'entrée en sommeil (DS partie B §5).
