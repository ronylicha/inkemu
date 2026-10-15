# SP2b — Note de passation

Plan : `docs/superpowers/plans/2026-10-15-sp2b-soc-peripherals.md` (2b-R1 à 2b-R7). Spec :
`docs/superpowers/specs/2026-10-15-sp2b-soc-peripherals-design.md` (2b-D1 à 2b-D14, D10, D13 et D14 précisées par
2b-R2 à 2b-R4). Recherche : `docs/research/esp32c3-soc-peripherals-sp2b.md`. ADR 0010 (nouvel). Point de départ :
`main` à `194f286`.

## Résultat

Mesures du 2026-10-15, construction release de cette branche, ROM rev3, poste de développement.

| Firmware | Avant (`194f286`) | Après |
|---|---|---|
| CrossPoint 1.6.5 (`--firmware`, 5 s) | panique du watchdog d'interruption dans l'attente de l'ADC, deux vies ; avertissements GPIO, I2C0, SPI2, APB_SARADC ; `--strict-mmio` arrêté à `APB_SARADC.APB_TSENS_CTRL` ; 154 s d'hôte | aucun avertissement, aucune panique ; sonde I2C rejetée par NACK (`ESP_ERR_INVALID_STATE`, comme sur un X4), verdict X4, carte SD absente, écran sans réponse, boucle d'attente (`[MEM] Free` toutes les 10 s) ; 59 s d'hôte |
| CrossPoint 1.6.5, `--strict-mmio` | arrêt au premier registre non modélisé | **60 s de temps d'appareil sans arrêt** (30 s d'hôte, horloge CPU à 10 MHz en fin de course) : premier accès non modélisé au-delà de 60 s |
| Firmware d'origine 3.0.2 (`--flash`) | paniques en boucle, `rst:0x3` ; strict arrêté à `GPIO.PIN13` | lit ses boutons, trouve le bouton d'allumage relâché, **retourne en sommeil profond à 637 053 µs**, seul le réveil GPIO activé (`WAKEUP_ENA 0x4`) : un seul avertissement, celui de ce réveil (sous-projet 2d) ; strict s'y arrête |
| `soc_signals` (nouveau) | — | 5 fronts de BUSY comptés, trames SPI et sélections attendues, ADC à ±2 mV de 1 650 mV, sonde I2C `ESP_ERR_NOT_FOUND`, aucun diagnostic en strict ; `0x08d63d647e5a8a29` ; 2,7 s d'hôte |
| `adc_wait_panic` (modifié, 2b-R4) | `b217bc3b287080ae` | lecture `0xfff` (GPIO1 tirée haut au reset), puis attente d'ADC2, panique, core dump, seconde vie ; aucun avertissement ; `42e50c75581ae020` |

- Les deux constructions depuis zéro de `build.sh` donnent les mêmes empreintes (`SHA256SUMS.inputs-soc-signals`,
  `SHA256SUMS.inputs-adc-wait-panic`) ; `SHA256SUMS.output` inchangé.
- Inchangés, revérifiés en local : `reference_boot` (13), les 12 autres tests `run_cli reference_firmware` (CoreMark
  laissé à la CI), `console-script` (5), les trois fenêtres du lockstep (hello 6 813 949, nvs 7 680 555, nvs-ota
  7 701 890, usages épinglés), `real_firmware.rs` (2, mis à jour).
- CI : job `reference-boot`, variable `INKEMU_SOC_SIGNALS_APP`, nouvelle ligne
  `run-ignored-tests.sh 1 --release -p boards --test reference_signals`.

## Décisions prises sans l'utilisateur

2b-D1 à 2b-D14 de la spec et 2b-R1 à 2b-R7 du plan. Les plus engageantes :

- les quatre traits et le branchement par broches (ADR 0010) : SP2d ne fait que câbler ;
- pas de GDMA, ni DMA de SPI2 (refusé par nom) : CrossPoint n'en use pas ;
- le balayage minuté du bootloader d'ESP-IDF accepté sans effet (2b-R2) ;
- le test de `soc_signals` dans `boards` (2b-R3) ;
- `adc_wait_panic` modifié pour garder la panique en CI (2b-R4) ;
- aucune fenêtre de lockstep nouvelle : QEMU n'a ni IO MUX, ni SPI2, ni I2C0, ni SAR ADC, et son GPIO lit 0 (2b-D12).

## Hypothèses (`ASSUMPTION:` du code)

- IO : alimentation des broches 3,3 V (`PAD_SUPPLY_MV`) ; reset de toutes les broches à `0xB00` du SVD (tirage haut,
  contre la table 5.12-1 du TRM) ; deux puces sélectionnées sur la même MISO donnent le ET ; un niveau (types 4 et 5)
  relève `STATUS` à chaque évaluation ; un front ne se voit qu'à l'évaluation qui le trouve ; les strobes `W1TS/W1TC`
  lisent 0 ; un périphérique valide la sortie d'un signal qu'il pilote ; une broche ni pilotée ni tirée lit bas, les
  deux tirages lisent haut ; un signal pris directement de l'IO MUX dont la broche a une autre fonction lit bas ;
  `IN_SEL` 22 à 29 lit bas.
- SPI2 : MOSI haut pendant une phase de lecture en half duplex ; `CS_SETUP`/`CS_HOLD` sans durée ; une transaction
  envoie les `W0..W15` du départ.
- I2C0 : un NACK avec `ack_en` arrête la liste et le STOP qui suit ne lève pas `TRANS_COMPLETE` ; FIFO RX vide lit 0
  (et `RXFIFO_UDF`) ; un octet écrit dans une FIFO TX pleine est perdu ; les impulsions de déblocage finissent
  aussitôt ; les interruptions de seuil montent quand une séquence ou un accès laisse la FIFO de l'autre côté.
- SAR ADC : la puce émulée est la puce typique de la courbe d'ESP-IDF pour son eFuse ; un canal absent lit 0 V ; une
  sortie inversée est le complément sur 12 bits ; la partie fractionnaire du diviseur n'ajoute rien ; un départ sans
  unité ne convertit rien.

## Laissé aux héritiers

- **SP2d (câblage du X4)** : brancher les puces de SP2a par `attach_*` ; le réveil du sommeil profond par GPIO3 (le
  firmware d'origine s'y arrête), le maintien des broches (`PAD_HOLD`) et l'isolement en sommeil ; les résistances de
  tirage de la carte (une ligne CS tirée haut est un `GpioInput`) ; le journal des entrées de l'utilisateur sur une
  puce (un bouton pressé) dans une session, `inputs_changed` étant l'appel du changement ; les registres de l'IO MUX et
  du SAR ADC acceptés sans effet qui lui sont donnés (table). Un SPI « bit-bang » par GPIO n'atteint pas un
  `SpiTarget` : une puce qui le veut écoute ses broches en `GpioOutput`.
- **SP2a (puces)** : une sélection sans trame est normale (2b-R6) ; `transfer` reçoit la MISO préremplie de `0xFF`.
- **SP3 (X3)** : les cibles I2C (aujourd'hui toute adresse reçoit un NACK) et les registres d'I2C0 qui lui sont donnés.
- **Non fait, non demandé** : GDMA, LEDC, RMT, I2S ; SPI2 dual/quad, DMA, phase DUMMY, esclave ; I2C esclave ; le
  balayage multi-canaux visible ; le capteur de température (registres stockés).
- La fenêtre `crosspoint` du lockstep (locale) finit toujours à `APB_SARADC.APB_TSENS_CTRL`, que QEMU n'a pas : la
  prolonger demande une ligne pour le capteur de température, que ni QEMU ni inkemu ne mesure (inchangé depuis SP1b-4d).

## Limites de preuve

- **Vérifié en local** : `cargo fmt --all --check` ; `tools/ci/clippy.sh` (`-D warnings`) sur `soc-esp32c3`,
  `emu-core`, `emu-cli`, `boards` ; rustdoc (`-D warnings`, éléments privés) sur `soc-esp32c3` et `emu-core` ; tests
  unitaires de `soc-esp32c3` (694), `emu-core`, `boards`, `emu-cli` (bibliothèque) ; la table des registres acceptés ;
  les suites ignorées listées plus haut ; `build.sh` entier deux fois dans `espressif/idf:v5.5.2` ; les trois fenêtres
  du lockstep de la CI contre QEMU et la fenêtre locale `crosspoint` (`cargo test --release -p inkemu-lockstep --test
  crosspoint -- --ignored` : 31 088 420 instructions, fin et usages épinglés inchangés, 1 383 s), un conteneur à la
  fois.
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace`, la couverture, le test CoreMark, la construction
  WebAssembly.
- Rien n'a été comparé à une puce : les durées des transactions (horloges de `CLOCK`, de `SCL_*`, de `SAR_CLK`) et les
  hypothèses ci-dessus sont lues dans le TRM et ESP-IDF, pas mesurées.

## Cartographie

Fichiers et types à reporter dans la cartographie du projet :

- `architecture_overview.md` : nouveau contrat « signaux vers les puces externes » (ADR 0010) : `emu-core::signals`
  (`Level`, `Nanos`, `GpioInput`, `GpioOutput`, `SpiTarget`, `AnalogSource`, impl. pour `Rc<RefCell<T>>`, pas de
  `Send`) ; le SoC résout ce que porte chaque broche ; `Esp32C3::attach_input/attach_output/attach_spi/attach_analog`
  et `inputs_changed` ; `AttachError`, `SpiWires` réexportés à la racine de `soc_esp32c3`.
- `backend_models.md` : `emu_core::{Level, Nanos}` et les quatre traits ; `soc_esp32c3::{AttachError (NoSuchPin,
  InputTaken, NoAnalogChannel, AnalogTaken), SpiWires {sclk, mosi, miso, cs, dc}}` ; `diag::{Spi2Refusal (Dma,
  MultiLine, Dummy, ThreeWire, CommandLength, AddressLength, DataLength, Slave, Segmented), I2cRefusal (Slave,
  NonFifo, Clock), AdcRefusal (Scan, BothUnits, Clock)}` et les `Unsupported::{PadSignal, PadInterrupt, Spi2, I2c,
  Adc}` ; `ctx::Module::{Spi2, I2c0, ApbSaradc}`, `ctx::Event::{Pins, Spi2, I2c0, SarAdc}`.
- `backend_services.md` : `periph/io/` (`IoMatrix` : GPIO et IO MUX, `pad.rs` résolution des broches et niveaux de
  repos des signaux, interruptions GPIO source 16, `Event::Pins`, `spi_transfer`, `millivolts`) remplace
  `periph/gpio.rs` et `periph/io_mux.rs` (supprimés) ; `periph/spi2.rs` (GP-SPI2 maître CPU, source 19) ;
  `periph/i2c0.rs` (I2C0 maître sans cible, source 29) ; `periph/saradc.rs` (SAR ADC ponctuel, `Calibration` depuis le
  bloc 2 d'eFuse, source 43, balayage minuté vers GDMA accepté) ; `efuse::system_data()` ; `SYSTEM.PERIP_RST_EN0`
  bits 6, 7, 28.
- `api_routes.md` : test ignoré `cargo test --release -p boards --test reference_signals -- --ignored`
  (`INKEMU_SOC_SIGNALS_APP`) ; effectif CI `reference-boot` +1 ligne (1 test) ; `real_firmware.rs` renommé
  (`crosspoint_1_6_5_x3_x4_runs_to_its_idle_loop_without_a_warning`,
  `the_stock_x4_firmware_goes_back_to_deep_sleep_without_its_power_button`).
- `integrations.md` : firmware de référence `soc_signals` (Apache-2.0, console USB-Serial-JTAG,
  `SHA256SUMS.inputs-soc-signals`) ; `adc_wait_panic` réépinglé ; variable `INKEMU_SOC_SIGNALS_APP` ; `boards`
  dev-dépend d'`emu-core` et `sha2` ; ADR 0010.
- `database_schema.md` : rien (aucun état persistant nouveau).
- `frontend_spa.md` (SP2c) : les puces branchées et `inputs_changed` sont la porte des boutons de l'interface.
- `tech_debt.md` : les héritiers ci-dessus (SP2d, SP2a, SP3) ; la fenêtre `crosspoint` du lockstep.
