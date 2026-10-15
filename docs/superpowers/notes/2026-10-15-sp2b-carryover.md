# SP2b — Note de passation

Plan : `docs/superpowers/plans/2026-10-15-sp2b-soc-peripherals.md` (2b-R1 à 2b-R13). Spec :
`docs/superpowers/specs/2026-10-15-sp2b-soc-peripherals-design.md` (2b-D1 à 2b-D14, D10, D13 et D14 précisées par
2b-R2 à 2b-R4). Recherche : `docs/research/esp32c3-soc-peripherals-sp2b.md`. ADR 0012 (nouvel). Point de départ :
`main` à `194f286`, fusionné ensuite avec `main` à `7d3e56f` (SP1b-4f, SP2a).

Numérotation des ADR : 0010 est celui de SP1b-4f (mode utilisateur et PMP), 0011 celui de SP2c (PR #17, l'application
Tauri), mergé avant cette branche ; celui de SP2b est donc 0012. Un dépôt qui n'a pas encore SP2c voit un trou entre
0010 et 0012 : il n'y manque rien.

## Résultat

Mesures du 2026-10-15, construction release de cette branche, ROM rev3, poste de développement.

| Firmware | Avant (`194f286`) | Après |
|---|---|---|
| CrossPoint 1.6.5 (`--firmware`, 5 s) | panique du watchdog d'interruption dans l'attente de l'ADC, deux vies ; avertissements GPIO, I2C0, SPI2, APB_SARADC ; `--strict-mmio` arrêté à `APB_SARADC.APB_TSENS_CTRL` ; 154 s d'hôte | aucun avertissement, aucune panique ; sonde I2C rejetée par NACK (`ESP_ERR_INVALID_STATE`, comme sur un X4), verdict X4, carte SD absente, écran sans réponse, boucle d'attente (`[MEM] Free` toutes les 10 s) ; 59 s d'hôte |
| CrossPoint 1.6.5, `--strict-mmio` | arrêt au premier registre non modélisé | **60 s de temps d'appareil sans arrêt** (30 s d'hôte, horloge CPU à 10 MHz en fin de course) : premier accès non modélisé au-delà de 60 s |
| Firmware d'origine 3.0.2 (`--flash`) | paniques en boucle, `rst:0x3` ; strict arrêté à `GPIO.PIN13` | lit ses boutons, trouve le bouton d'allumage relâché, **retourne en sommeil profond à 637 053 µs**, seul le réveil GPIO activé (`WAKEUP_ENA 0x4`) : un seul avertissement, celui de ce réveil (sous-projet 2d) ; strict s'y arrête |
| `soc_signals` (nouveau) | — | 5 fronts de BUSY comptés, trames SPI et sélections attendues, ADC à ±2 mV de 1 650 mV, sonde I2C `ESP_ERR_NOT_FOUND`, aucun diagnostic en strict ; `0x08d63d647e5a8a29` ; 2,7 s d'hôte |
| `adc_wait_panic` (modifié, 2b-R4) | `b217bc3b287080ae` | lecture `0xfff` (GPIO1 tirée haut au reset), puis effacement du bit de fin d'ADC2 et attente d'ADC2, panique, core dump, seconde vie ; aucun avertissement ; `20e53f8cdf1823fc` (2b-R11 ; `42e50c75581ae020` avant l'effacement) |

- Les deux constructions depuis zéro de `build.sh` donnent les mêmes empreintes (`SHA256SUMS.inputs-soc-signals`,
  `SHA256SUMS.inputs-adc-wait-panic`) ; `SHA256SUMS.output` inchangé.
- Inchangés, revérifiés en local : `reference_boot` (13), les 12 autres tests `run_cli reference_firmware` (CoreMark
  laissé à la CI), `console-script` (5), les trois fenêtres du lockstep (hello 6 813 949, nvs 7 680 555, nvs-ota
  7 701 890, usages épinglés), `real_firmware.rs` (2, mis à jour).
- CI : job `reference-boot`, variable `INKEMU_SOC_SIGNALS_APP`, nouvelle ligne
  `run-ignored-tests.sh 1 --release -p boards --test reference_signals`.

## Décisions prises sans l'utilisateur

2b-D1 à 2b-D14 de la spec et 2b-R1 à 2b-R13 du plan. Les plus engageantes :

- les cinq traits et le branchement par broches (ADR 0012) : SP2d ne fait que câbler ;
- `Clocked` et `attach_clocked` : une puce avancée en temps d'appareil, une fois par instant au plus (2b-R8) ;
- un refus finit l'opération refusée, partout comme sur SPI2 (2b-R9) ;
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
  aussitôt ; les interruptions de seuil montent quand une séquence ou un accès laisse la FIFO de l'autre côté ; un
  `TRANS_START` pendant une séquence est ignoré (refus nommé `StartWhileRunning`, le TRM ne le définit pas) ;
  `FSM_RST` abandonne la séquence en cours sans en montrer les effets, le bus inactif, FIFO et interruptions gardées.
- SAR ADC : la puce émulée est la puce typique de la courbe d'ESP-IDF pour son eFuse ; un canal absent lit 0 V ; une
  sortie inversée est le complément sur 12 bits ; la partie fractionnaire du diviseur n'ajoute rien ; un départ sans
  unité ne convertit rien.

## Pour SP2d : le contrat des puces

Les cinq traits d'`emu_core::signals` et les points d'entrée d'`Esp32C3` (ADR 0012) :

```rust
pub enum Level { Low, High }
pub struct Nanos(pub u64);                 // temps d'appareil depuis la mise sous tension
pub trait GpioInput { fn level(&mut self, at: Nanos) -> Option<Level>; fn next_change(&mut self, at: Nanos) -> Option<Nanos> { None } }
pub trait GpioOutput { fn driven(&mut self, level: Option<Level>, at: Nanos); }
pub trait SpiTarget { fn select(&mut self, selected: bool, at: Nanos); fn transfer(&mut self, dc: Option<Level>, mosi: &[u8], miso: &mut [u8], at: Nanos); }
pub trait AnalogSource { fn millivolts(&mut self, at: Nanos) -> u32; }
pub trait Clocked { fn next_deadline(&mut self, at: Nanos) -> Option<Nanos>; fn advance(&mut self, at: Nanos); }
// tous implémentés pour Rc<RefCell<T>> ; aucun n'est Send

Esp32C3::attach_input(pin, Box<dyn GpioInput>) -> Result<(), AttachError>
Esp32C3::attach_output(pin, Box<dyn GpioOutput>) -> Result<(), AttachError>
Esp32C3::attach_spi(SpiWires { sclk, mosi, miso, cs, dc }, Box<dyn SpiTarget>) -> Result<(), AttachError>
Esp32C3::attach_analog(pin /* 0..=5 */, Box<dyn AnalogSource>) -> Result<(), AttachError>
Esp32C3::attach_clocked(Box<dyn Clocked>)
Esp32C3::inputs_changed()
```

- **Le temps.** Une puce qui travaille d'elle-même (le SSD1677 : `advance` et `next_deadline` de SP2a) se branche
  aussi par `attach_clocked`, partagée avec ses lignes par `Rc<RefCell<_>>`. Le SoC lui demande son échéance au
  branchement et après chaque appel qu'il fait à une puce, arme une échéance dans son ordonnanceur en temps
  d'appareil, et l'y avance, que le CPU exécute, attende une interruption, dorme jusqu'à un timer ou que la puce soit
  en sommeil profond (la carte reste alimentée ; seules les puces cadencées sont avancées, sans évaluer les broches,
  et le sommeil ne finit faute de réveil qu'une fois qu'aucune n'a plus d'échéance) ; puis il relit les broches
  (BUSY) et lève l'interruption GPIO sur front. Avant tout autre appel à une puce, il avance d'abord celles dont
  l'échéance est venue. Une puce n'est avancée qu'une fois par instant : une échéance qui n'est pas après cet instant
  est reprise au cycle suivant de la base de temps (pas de boucle sur un cycle ; une puce toujours due est avancée à
  chaque cycle). L'instant passé à `advance` est exactement le premier cycle de la base de temps (6,25 ns) à
  l'échéance ou après quand le CPU attend ; au plus tard la fin de l'instruction en cours quand il exécute. Une puce
  toujours due empêche un sommeil profond de finir faute de réveil (seuls `--max-time` ou le domaine RTC l'arrêtent)
  et, restée due après 1 000 avances de suite (`STUCK_CLOCKED_INSTANTS`), elle est nommée une fois
  (`Diagnostic::ClockedChipStuck`, un avertissement qui n'arrête pas une exécution stricte). Une échéance au même
  instant que l'appel RTC avance la puce une fois, puis le réveil fait son reset.
- **`next_change`** n'est demandé que si une interruption GPIO est configurée sur la broche ; sans elle, un niveau se
  relit à la lecture suivante. Le temps propre d'une puce passe par `Clocked`, pas par `next_change`.
- **SPI.** Une sélection peut venir sans trame (2b-R6) ; avec le CS matériel, sélection, transfert et désélection sont
  au même instant ; `miso` arrive prérempli de `0xFF` ou `0x00` selon le niveau de la ligne d'entrée. La lecture
  demi-duplex du panneau est du bit-bang : `GpioOutput` sur SCLK et MOSI, `GpioInput` sur MOSI.
- **Les refus** finissent l'opération (fin de transaction de SPI2, `TRANS_COMPLETE` d'I2C0, fin des deux unités du
  SAR ADC) : un firmware ne reste jamais suspendu à un bit qui ne monterait pas. Un second départ pendant une
  opération (I2C0 `StartWhileRunning`, SAR ADC `StartWhileConverting`) est refusé par nom et l'opération en cours
  garde sa fin.

## Laissé aux héritiers

- **SP2d (câblage du X4)** : brancher les puces de SP2a par `attach_*` ; le réveil du sommeil profond par GPIO3 (le
  firmware d'origine s'y arrête), le maintien des broches (`PAD_HOLD`) et l'isolement en sommeil ; les résistances de
  tirage de la carte (une ligne CS tirée haut est un `GpioInput`) ; le journal des entrées de l'utilisateur sur une
  puce (un bouton pressé) dans une session, `inputs_changed` étant l'appel du changement ; les registres de l'IO MUX et
  du SAR ADC acceptés sans effet qui lui sont donnés (table). Un SPI « bit-bang » par GPIO n'atteint pas un
  `SpiTarget` : une puce qui le veut écoute ses broches en `GpioOutput` et répond par un `GpioInput` ; c'est le cas de
  la lecture demi-duplex du panneau du X4 (le firmware repasse MOSI en entrée et bat SCLK par GPIO : `GpioOutput` sur
  SCLK et MOSI, `GpioInput` sur MOSI). Voir « Pour SP2d » ci-dessous.
- **SP2a (puces)** : une sélection sans trame est normale (2b-R6) ; `transfer` reçoit la MISO préremplie de ce que lit
  la ligne d'entrée du maître si la puce ne pilote rien : `0xFF` quand elle lit haut (tirage vers le haut), `0x00`
  quand elle lit bas (broche sans tirage ou tirée bas). Avec le CS matériel, `select(true)`, `transfer` et
  `select(false)` arrivent au même instant. La note de SP2a a reçu une révision datée qui renvoie ici (lecture
  demi-duplex du panneau, échéance par `Clocked`).
- **SP3 (X3)** : les cibles I2C (aujourd'hui toute adresse reçoit un NACK) et les registres d'I2C0 qui lui sont donnés.
- **Non fait, non demandé** : GDMA, LEDC, RMT, I2S ; SPI2 dual/quad, DMA, phase DUMMY, esclave ; I2C esclave ; le
  balayage multi-canaux visible ; le capteur de température (registres stockés).
- La fenêtre `crosspoint` du lockstep (locale) finit toujours à `APB_SARADC.APB_TSENS_CTRL`, que QEMU n'a pas : la
  prolonger demande une ligne pour le capteur de température, que ni QEMU ni inkemu ne mesure (inchangé depuis SP1b-4d).

## Limites de preuve

- **Vérifié en local** : `cargo fmt --all --check` ; `tools/ci/clippy.sh` (`-D warnings`) sur `soc-esp32c3`,
  `emu-core`, `emu-cli`, `boards` ; rustdoc (`-D warnings`, éléments privés) sur `soc-esp32c3` et `emu-core` ; tests
  unitaires de `soc-esp32c3` (706 après les deux relectures), `emu-core`, `boards`, `emu-cli` (bibliothèque) ; la table des registres acceptés ;
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

- `architecture_overview.md` : nouveau contrat « signaux vers les puces externes » (ADR 0012) : `emu-core::signals`
  (`Level`, `Nanos`, `GpioInput`, `GpioOutput`, `SpiTarget`, `AnalogSource`, `Clocked`, impl. pour
  `Rc<RefCell<T>>`, pas de `Send`) ; le SoC résout ce que porte chaque broche ;
  `Esp32C3::attach_input/attach_output/attach_spi/attach_analog/attach_clocked` et `inputs_changed` ; une puce
  `Clocked` est avancée à ses échéances en temps d'appareil, même pendant un `WFI` ; `AttachError`, `SpiWires`
  réexportés à la racine de `soc_esp32c3` ; ADR 0012 (0011 est celui de SP2c).
- `backend_models.md` : `emu_core::{Level, Nanos}` et les cinq traits ; `soc_esp32c3::{AttachError (NoSuchPin,
  InputTaken, NoAnalogChannel, AnalogTaken), SpiWires {sclk, mosi, miso, cs, dc}}` ; `diag::{Spi2Refusal (Dma,
  MultiLine, Dummy, ThreeWire, CommandLength, AddressLength, DataLength, Slave, Segmented), I2cRefusal (Slave,
  NonFifo, Clock, StartWhileRunning), AdcRefusal (Scan, BothUnits, StartWhileConverting, Clock)}` et les `Unsupported::{PadSignal,
  PadInterrupt, Spi2, I2c, Adc}` ; `Diagnostic::ClockedChipStuck { chip }` et `diag::STUCK_CLOCKED_INSTANTS` (1 000) ;
  `ctx::Module::{Spi2, I2c0, ApbSaradc}`, `ctx::Event::{Pins, Spi2, I2c0, SarAdc}`.
- `backend_services.md` : `periph/io/` (`IoMatrix` : GPIO et IO MUX, `pad.rs` résolution des broches et niveaux de
  repos des signaux, interruptions GPIO source 16, `Event::Pins` pour les changements annoncés et les échéances des
  puces `Clocked` (`ClockedChip`, une avance par instant), `spi_transfer`, `millivolts`) remplace
  `periph/gpio.rs` et `periph/io_mux.rs` (supprimés) ; `periph/spi2.rs` (GP-SPI2 maître CPU, source 19) ;
  `periph/i2c0.rs` (I2C0 maître sans cible, source 29, `FSM_RST`, refus qui finissent la séquence) ; `periph/saradc.rs` (SAR ADC ponctuel, `Calibration` depuis le
  bloc 2 d'eFuse, source 43, balayage minuté vers GDMA accepté) ; `efuse::system_data()` ; `SYSTEM.PERIP_RST_EN0`
  bits 6, 7, 28.
- `api_routes.md` : test ignoré `cargo test --release -p boards --test reference_signals -- --ignored`
  (`INKEMU_SOC_SIGNALS_APP`) ; effectif CI `reference-boot` +1 ligne (1 test) ; `real_firmware.rs` renommé
  (`crosspoint_1_6_5_x3_x4_runs_to_its_idle_loop_without_a_warning`,
  `the_stock_x4_firmware_goes_back_to_deep_sleep_without_its_power_button`).
- `integrations.md` : firmware de référence `soc_signals` (Apache-2.0, console USB-Serial-JTAG,
  `SHA256SUMS.inputs-soc-signals`) ; `adc_wait_panic` réépinglé ; variable `INKEMU_SOC_SIGNALS_APP` ; `boards`
  dev-dépend d'`emu-core` et `sha2` ; ADR 0012.
- `database_schema.md` : rien (aucun état persistant nouveau).
- `frontend_spa.md` (SP2c) : les puces branchées et `inputs_changed` sont la porte des boutons de l'interface.
- `tech_debt.md` : les héritiers ci-dessus (SP2d, SP2a, SP3) ; la fenêtre `crosspoint` du lockstep.
