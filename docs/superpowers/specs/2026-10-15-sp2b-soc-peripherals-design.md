# SP2b — Les périphériques du SoC pour le X4 : design

Date : 2026-10-15. Spec globale : `docs/superpowers/specs/2026-10-11-inkemu-design.md` §1, §3.2 (« les puces externes
ne connaissent pas le SoC, elles ne voient que des signaux »), §3.3, §9 (sous-projet 2), §10.3 critère 2. Recherche :
`docs/research/esp32c3-soc-peripherals-sp2b.md` (« P » ci-dessous). ADR 0010 (nouvel), ADR 0003. Point de départ :
`main` à `194f286`.

## 1. But

Le sous-projet 2 rend le X4 visible et pilotable. SP2b en fournit la moitié côté SoC : les périphériques qui portent
les signaux vers l'écran, la carte SD, les boutons et la batterie, et le contrat neutre par lequel SP2d branchera les
puces de SP2a. Critère : CrossPoint 1.6.5 franchit l'attente du SAR ADC qui l'arrête aujourd'hui, et le premier accès
non modélisé de son démarrage est plus loin qu'au `194f286` ; un firmware de référence à nous prouve en CI GPIO,
GP-SPI2 (avec une cible du côté du test), les interruptions GPIO et le SAR ADC.

## 2. Décisions

| # | Décision | Raison |
|---|---|---|
| 2b-D1 | **Quatre traits dans `emu-core::signals`** : `GpioInput { level(at) -> Option<Level>, next_change(at) -> Option<Nanos> }`, `GpioOutput { driven(level: Option<Level>, at) }`, `SpiTarget { select(selected, at), transfer(dc, mosi, miso, at) }`, `AnalogSource { millivolts(at) -> u32 }` ; `Level`, `Nanos` (temps d'appareil en ns) ; implémentations pour `Rc<RefCell<T>>`. Pas de `Send`. | ADR 0010 : un contrat que ni le SoC ni les puces ne possèdent ; une puce à plusieurs lignes se partage. La machine n'est pas `Send` (`test_support::within`). |
| 2b-D2 | **Branchement par broches** (`Esp32C3::attach_input/output/spi/analog`, `inputs_changed`) ; le SoC résout avec ses registres ce que porte chaque broche ; erreurs typées (`AttachError`) pour une broche inexistante, une broche qui a déjà une puce en entrée, une broche sans canal ADC. | ADR 0010 : CS logiciel et matériel traités pareil ; SP2d n'écrit que le câblage. |
| 2b-D3 | **Un modèle `IoMatrix` répond à GPIO et à IO MUX**, propriétaire des registres, des signaux de sortie des périphériques modélisés et des branchements. Les registres `GPIO.ENABLE` et `FUNC0..5_IN_SEL_CFG` quittent la table des registres acceptés sans effet ; l'IO MUX y reste pour ce qu'il n'a pas d'effet (force, filtre, réglages de sommeil). | P §B ; ADR 0010 point 3. |
| 2b-D4 | **Résolution d'une broche** : pilotée par le SoC si la fonction IO MUX est GPIO (ou FSPI directe, fonction 2, ou U0TXD repos haut sur GPIO21 en fonction 0) et la sortie validée (`OEN_SEL`/`ENABLE` ou la validation du périphérique), drain ouvert ne tirant que vers le bas ; sinon le niveau d'une puce branchée ; sinon le tirage (`FUN_WPU` haut, `FUN_WPD` bas, les deux : haut) ; sinon bas. `GPIO.IN` lit 0 sans `FUN_IE`. Contention : le SoC l'emporte. Ces trois derniers points sont des `ASSUMPTION:` (P, questions 2 et 3). | TRM §5.4–5.5 ; `gpio_ll_get_level`. |
| 2b-D5 | **Signaux de sortie modélisés** : `OUT` (128), U0TXD (6, repos haut), I2C SCL/SDA (53, 54, relâchés), FSPICLK, FSPID, FSPIHD, FSPIWP (repos bas), FSPICS0–5 (repos haut). Un autre signal routé vers une broche est refusé par un avertissement nommé (`Unsupported::PadSignal`), la broche n'est pas pilotée. | Ce que la carte du X4 et nos firmwares utilisent ; rien de muet. |
| 2b-D6 | **Interruptions GPIO** : types 1 à 5, `STATUS` (avec `W1TS`/`W1TC`), `PCPU_INT`, source 16 ; un niveau tient `STATUS` levé tant qu'il dure (`ASSUMPTION:`, P question 1). L'activation NMI (`INT_ENA` hors bit 0) est refusée par nom. Relecture des entrées à l'instant `next_change` de leur puce (`Event::Pins`) quand une interruption est configurée. | FreeInk attend BUSY par une interruption sur les deux fronts (P §A). |
| 2b-D7 | **GP-SPI2 maître en mode CPU** : `CMD.UPDATE` (effacé aussitôt), `CMD.USR`, phases commande (8 ou 16 bits), adresse (8 à 32 bits par octets), données (1 à 64 octets, `W0..W15`, moitié haute), full et half duplex, ordre des bits, CS matériel 0–5 et `CS_KEEP_ACTIVE`, `DMA_INT_RAW.TRANS_DONE` et la source 19. Durée : les bits de la transaction à l'horloge de `CLOCK` (source APB ou quartz). Refusés par nom, la transaction se terminant sans rien envoyer : DMA (`DMA_TX_ENA`/`DMA_RX_ENA`), dual/quad, phase DUMMY, longueurs qui ne sont pas des octets, mode esclave, transfert segmenté. | P §C ; CrossPoint n'utilise que le mode CPU (P §A, §F). |
| 2b-D8 | **Pas de GDMA.** | CrossPoint ne l'utilise pas (P §A, §F) ; le DMA de SPI2 est refusé par nom (2b-D7). |
| 2b-D9 | **I2C0 maître** : commandes RSTART, WRITE, READ, STOP, END, FIFO de 32 octets par `DATA`, `SR`, `FIFO_ST`, interruptions et source 29. **Aucune cible** : l'octet d'adresse reçoit un NACK ; avec `ack_en`, NACK_INT, un STOP sur le bus et l'arrêt de la séquence ; sans, les octets suivants partent et un READ reçoit `0xFF`. Durée : les bits à l'horloge de `SCL_LOW/HIGH/WAIT_HIGH_PERIOD`. Refusés par nom : mode esclave, source RC_FAST, accès non-FIFO. | P §D : ce qu'un vrai X4 répond à la sonde X3/X4 ; les cibles I2C sont pour SP3 (X3). |
| 2b-D10 | **SAR ADC en lecture ponctuelle** (ADC1 canaux 0–4, ADC2 canal 0) : `ONETIME_START` sur front montant, 25 cycles de SAR_CLK, `INT_RAW` bit 31/30, `SAR1/2DATA_STATUS`, `SAR1/2_INV`, `INT_ENA/ST/CLR`, source 43. La lecture d'une tension est l'inverse de la calibration d'ESP-IDF (droite des eFuse puis courbe d'erreur), si bien que `analogReadMilliVolts` rend la tension de la source (`ASSUMPTION:`, P question 7). Une broche sans source vaut 3300 mV si le SoC la pilote haut ou si elle est tirée haut, 0 sinon. Le balayage multi-canaux est refusé par nom. Les registres du capteur de température sont stockés. | P §E ; CrossPoint lit ses boutons en brut et sa batterie en mV. |
| 2b-D11 | **Resets** : `PERIP_RST_EN0` bits 6 (SPI2), 7 (I2C0), 28 (APB_SARADC) remettent leur module à zéro ; un reset du cœur remet les registres de l'IO MUX et de la matrice à zéro et garde les branchements, les puces voyant leurs lignes relâchées. | TRM §6.1.3 ; `system_reg.h`. |
| 2b-D12 | **Lockstep : aucune fenêtre.** Le GPIO de QEMU lit 0 hors `STRAP` et ignore les écritures ; IO MUX, SPI2, I2C0 et le SAR ADC n'existent pas dans QEMU. Les fenêtres existantes ne lisent aucun registre que SP2b change ; leurs comptes et usages épinglés ne bougent pas. | P §G, §H. |
| 2b-D13 | **Firmware de référence `soc_signals`** (Apache-2.0, ESP-IDF, construit dans `espressif/idf:v5.5.2`, épinglé) : GPIO en sortie et en entrée, une interruption sur les deux fronts, deux périphériques SPI (CS matériel ; CS et DC logiciels), une lecture ADC calibrée ; un test ignoré de `soc-esp32c3` le démarre derrière le bootloader livré avec des cibles branchées du côté du test, dans le job `reference-boot`. | Preuve en CI, sans fichier tiers. |
| 2b-D14 | **Ce qui change d'empreinte** : seul `adc_wait_panic` (son attente de l'ADC se termine désormais) ; son test garde sa première vie et prouve la réponse de l'ADC. Les autres empreintes épinglées (12 s, 40 000 001 cycles, `deep_sleep_timer`, CoreMark) et les fenêtres du lockstep ne bougent pas. | Un changement d'empreinte doit avoir une raison écrite. |

## 3. Hors périmètre

- Le câblage du X4 dans `boards` et les puces (SP2d, SP2a) ; l'application (SP2c).
- Le réveil du sommeil profond par GPIO3, le maintien des broches (`PAD_HOLD`) et l'isolement en sommeil (SP2d).
- GDMA, LEDC, RMT, I2S ; les cibles I2C (SP3).
- Le journal des entrées de l'utilisateur sur une puce (un bouton pressé) : SP2d/SP2c.

## 4. Critères d'acceptation

1. Les traits de 2b-D1 existent, documentés, testés ; `cargo doc` sans avertissement.
2. Les modèles de 2b-D3 à 2b-D11 ont leurs tests unitaires ; la table des registres acceptés sans effet est à jour.
3. `soc_signals` se construit à ses empreintes épinglées (deux constructions identiques) ; son test passe en local et
   dans le job `reference-boot` (effectifs mis à jour).
4. CrossPoint 1.6.5 en local : plus d'attente de l'ADC sans fin ; le premier accès non modélisé est noté dans la note
   de passation.
5. Les empreintes épinglées et les fenêtres du lockstep ne changent que comme 2b-D14 le dit ; la CI est verte.
