# inkemu — Spécification de conception

- **Date** : 2026-10-11
- **Statut** : en relecture
- **Portée de ce document** : architecture globale (contrats communs à tous les sous-projets) + spécification détaillée du **sous-projet 1**. Chaque sous-projet suivant aura sa propre spec.
- **Recherche matérielle de référence** : [`docs/research/xteink-hardware.md`](../../research/xteink-hardware.md) (sources et niveau de confiance pour chaque fait).

---

## 1. Objectif

inkemu est un émulateur **au niveau binaire** d'appareils embarqués à écran e-ink. Il exécute le firmware réel (`.bin`) sans recompilation, sur un appareil virtuel affiché à l'écran, pour vérifier le rendu et le fonctionnement d'un firmware avant de le flasher.

Appareils cibles initiaux : **Xteink X4, X3, X4 Pro, X4C**. Le projet est conçu pour accueillir d'autres appareils par simple ajout d'un profil.

### 1.1 Besoins exprimés

| Besoin | Conséquence de conception |
|---|---|
| Tester le firmware officiel, les firmwares communautaires (CrossPoint, Papyrix…) et un firmware personnel | Émulation binaire : aucune hypothèse sur la disponibilité des sources ou des symboles |
| Réalisme maximal : mémoire disponible, vitesse CPU, rendu | Carte mémoire exacte, modèle de cycles, émulation des contrôleurs (pas de l'image) |
| Timing « cycle-exact » | Architecture prête pour le cycle-exact ; modèle livré cycle-approché et recalé (voir §1.3) |
| Desktop (Linux, macOS, Windows) **et** web | Cœur Rust compilé en natif et en WASM ; UI web partagée (Tauri sur desktop) |
| Rendu e-ink réaliste, moniteur de ressources, debug, tests automatisés | Voir §5 et §6 |
| Carte SD : dossier hôte et image `.img` | Deux back-ends de stockage derrière une même interface |
| Wi-Fi avec un vrai accès Internet | Émulation du MAC radio + point d'accès virtuel + NAT vers l'hôte |
| BLE : appareils virtuels et BLE réel du PC | Appareils virtuels partout ; passerelle vers l'adaptateur hôte sur desktop |
| Tactile du X4 Pro | Saisie fidèle (tap, swipe, pinch), overlay des touchers, gestes scriptables |
| Interface FR/EN, extensible à d'autres appareils | i18n dès le départ ; appareils décrits par profils déclaratifs |

### 1.2 Hors périmètre

- Recompiler un firmware contre un faux pilote (c'est ce que font les simulateurs existants ; inkemu vise l'inverse).
- Déchiffrer les mises à jour `.xota` chiffrées : elles sont refusées avec un message explicite. Un dump flash ou une image en clair est requis.
- Afficher les zones tactiles déclarées par l'application : elles vivent dans la mémoire du firmware et ne sont pas observables au niveau matériel.
- Un JIT : il rend le comptage de cycles approximatif par blocs et il est très contraint en WASM.

### 1.3 Fidélité temporelle : ce qui est promis

Espressif ne publie pas la micro-architecture de ses cœurs (pipeline, latences, arbitrage de bus), et aucun appareil n'est disponible pour mesurer. Un modèle « cycle-exact » ne peut donc pas être **vérifié** aujourd'hui. Les engagements sont les suivants :

1. Le modèle de timing est un composant **interchangeable** (`TimingModel`), indépendant du cœur d'exécution. Rien dans l'architecture n'empêche d'affiner jusqu'au cycle.
2. Le modèle livré est **cycle-approché** : coûts par classe d'instruction tirés de la documentation, simulation du cache flash, durées réelles des périphériques. Il est recalé sur des références publiques mesurées (score CoreMark publié de l'ESP32-C3).
3. Un **banc de calibration** (sous-projet 8) permet à quiconque dispose d'un appareil de produire un modèle calibré sans toucher au code.
4. L'UI affiche toujours le statut du modèle actif : « non calibré sur matériel » ou « calibré (appareil, date) ».

---

## 2. Matériel cible (résumé)

| Modèle | SoC | ISA | Mémoire | Écran | Spécificités |
|---|---|---|---|---|---|
| X4 | ESP32-C3 | RV32IMC, 160 MHz | 400 Ko SRAM, flash 16 Mo, pas de PSRAM | 800×480, SSD1677 | SD en SPI sur le bus de l'écran ; boutons sur 2 échelles ADC ; batterie sur ADC ; pas de RTC |
| X3 | ESP32-C3 | RV32IMC | idem | 792×528, UC8253/UC8279d | I2C : BQ27220, DS3231, QMI8658 ; sonde I2C au boot pour distinguer X3/X4 |
| X4 Pro | ESP32-S3 | Xtensa LX7 ×2 | 8 Mo PSRAM octale, flash 16 Mo | 800×480, SSD1677 | Tactile GT911, SDMMC 1-bit, éclairage chaud/froid, RTC BM8563, jauge CW2017 |
| X4C | ESP32-S3 | Xtensa LX7 ×2 | 8 Mo PSRAM | 800×480, SSD1677/UC8179/UC8279 selon révision | 8 boutons, SDMMC, BM8563, CW2017 |

Les brochages complets, les tables de partitions et le format des images sont dans la recherche de référence.

**Points matériels non vérifiés** (à traiter comme paramètres de profil, pas comme constantes) : polarité de BUSY et fréquence SPI de l'écran du X4 (les sources se contredisent ; on suit le code des firmwares : BUSY actif haut, 10 MHz), circuits de charge, référence exacte du panneau X3, état réel des eFuses des appareils commerciaux.

---

## 3. Architecture globale

### 3.1 Principe : une machine déterministe

Un seul fil d'exécution fait avancer un **temps virtuel mesuré en cycles** du SoC. Toute entrée extérieure (bouton, toucher, heure, paquet réseau, événement BLE, insertion USB) entre comme un **événement horodaté en cycles** dans le `Scheduler`.

Conséquences :

- une session peut être **enregistrée puis rejouée** à l'identique, y compris le trafic réseau, dont les réponses font partie de l'enregistrement ;
- la CI est reproductible ;
- le multi-cœur du S3 est simulé par entrelacement déterministe des deux cœurs, pas par deux threads hôtes.

### 3.2 Crates du workspace Rust

**Toolchain** : Rust **1.99.0** (dernière stable au 2026-10-11), édition 2024, épinglée dans `rust-toolchain.toml`. Chaque nouvelle stable est adoptée dans une PR dédiée, qui passe par la CI complète.

| Crate | Responsabilité | Dépendances |
|---|---|---|
| `emu-core` | Contrats : `Cpu`, `Bus`, `Peripheral` (accès MMIO + `tick`), `Scheduler`, `TimingModel`, `EventLog` (enregistrement/rejeu) | — |
| `cpu-rv32` | RV32IMC + Zicsr + extensions et CSR spécifiques ESP32-C3 | `emu-core` |
| `cpu-xtensa` | Xtensa LX7 (fenêtres de registres, exceptions, options du S3) | `emu-core` |
| `svd-regs` | Définitions de registres générées depuis les fichiers SVD officiels d'Espressif (adresses, champs, valeurs de reset) | — |
| `soc-esp32c3` / `soc-esp32s3` | Carte mémoire, ROM, cache/MMU XIP, horloges, interruptions, timers, GPIO matrix, UART, USB-Serial-JTAG, SPI, GDMA, ADC, I2C, LEDC, eFuse, RTC_CNTL, MAC radio | `emu-core`, CPU, `svd-regs` |
| `chips` | Puces externes : contrôleurs e-ink, carte SD (SPI et SDMMC), jauges, RTC, IMU, tactile, échelle de boutons, modèle de batterie | `emu-core` |
| `boards` | Chargement et validation des **profils TOML** ; instanciation et câblage d'un appareil | SoC, `chips` |
| `firmware` | Lecture des images ESP-IDF, détection des formats refusés, assemblage de la flash | — |
| `storage` | Back-ends SD : dossier hôte exposé en FAT32 virtuel, image `.img` brute | — |
| `netstack` | Point d'accès virtuel, DHCP, DNS, NAT vers l'hôte ; transport natif (sockets) ou WebSocket (web) | — |
| `ble-bridge` | Appareils BLE virtuels ; passerelle vers l'adaptateur hôte (desktop) | — |
| `emu-cli` | Exécution headless, scripts d'entrées, captures, comparaison d'images, serveur GDB, enregistrement/rejeu | tout |
| `app` | UI TypeScript partagée ; enveloppe Tauri (desktop, cœur natif) et build web (cœur WASM dans un Web Worker) | tout |

Règles de dépendance : `emu-core` ne dépend de rien ; les puces externes ne connaissent pas le SoC, elles ne voient que des signaux (SPI, I2C, GPIO, ADC) ; seul `boards` relie SoC et puces.

### 3.3 Profils d'appareil

Un appareil est un fichier TOML. Il déclare :

- le SoC et sa révision ;
- la taille de flash et de PSRAM ;
- la liste des puces externes et leurs paramètres (contrôleur et dimensions de l'écran, adresses I2C) ;
- le **câblage** (signal ↔ GPIO, bus partagés, polarités) ;
- les valeurs ADC des échelles de boutons ;
- les tables de partitions proposées ;
- la géométrie de l'habillage graphique (position de l'écran et des boutons sur l'image de l'appareil).

Ajouter un appareil revient à écrire un profil, plus le modèle d'une puce si elle est inédite. Le chargement d'un profil est validé : signal non câblé, conflit de broches ou puce inconnue sont refusés avec un message qui désigne la ligne fautive.

### 3.4 Dépendances externes

| Élément | Origine | Décision |
|---|---|---|
| ROM interne ESP32-C3/S3 | Espressif (fournie dans les releases du fork QEMU / dépôt `esp-rom-elfs`) | **À vérifier en tout premier (SP1)** : licence de redistribution. Si elle est permise, la ROM est embarquée ; sinon elle est téléchargée au premier lancement depuis la source officielle, avec vérification d'empreinte SHA-256 épinglée. |
| Bootloader de second étage | ESP-IDF, Apache-2.0 | Embarqué, un par puce, construit de façon reproductible (Docker `espressif/idf`, version épinglée) |
| Fichiers SVD | Espressif | Licence à vérifier avec la ROM ; générés en Rust au build |
| QEMU (fork Espressif) | GPL | **Uniquement outil de test** (oracle lockstep), jamais lié ni distribué |

---

## 4. Cœur d'exécution et timing

### 4.1 Interpréteur

- Décodage une fois par instruction, mis en cache par page. Le cache est invalidé quand une page est écrite (IRAM, mise à jour OTA, changement de mapping MMU).
- Quand le CPU exécute `WFI` ou attend un événement, l'émulateur **saute au prochain événement planifié** au lieu de simuler l'attente.
- Si l'hôte est trop lent pour le temps réel, le temps virtuel ralentit mais reste exact. Le firmware ne voit aucune différence ; l'UI affiche le rapport de vitesse (« ×0,7 »).

### 4.2 Modèle de timing livré (`DatasheetTiming`)

- Coût par classe d'instruction : ALU, multiplication, division, chargement, stockage, branche prise ou non prise, saut.
- **Cache flash simulé** avec la géométrie réelle du C3 (I-cache 16 Ko ; associativité et taille de ligne tirées du manuel technique). Un défaut coûte la durée calculée d'une lecture SPI DIO à 80 MHz.
- Accès SRAM et périphériques avec leurs états d'attente documentés.
- Recalage : un test automatique exécute CoreMark et vérifie que le score émulé reste dans **±15 %** du score publié pour l'ESP32-C3.

### 4.3 Mémoire

Carte d'adresses réelle, taille exacte de la SRAM, part réservée au cache déduite de la configuration écrite par le firmware. Tout accès hors zone mappée lève l'exception que lèverait le matériel ; l'émulateur la journalise.

### 4.4 Validation du cœur

- Suite officielle `riscv-arch-test` pour RV32IMC + Zicsr.
- **Mode lockstep** contre le fork QEMU d'Espressif : même image flash, même état initial, comparaison des registres généraux, du PC et des CSR clés après chaque instruction. La première divergence est rapportée avec l'instruction, l'état avant/après des deux côtés et les 64 instructions précédentes.

---

## 5. Démarrage, périphériques et affichage

### 5.1 Chaîne de boot

Reset → ROM → bootloader (`0x0`) → table de partitions (`0x8000`) → `otadata` → application. Aucun raccourci : temps de boot, bascule OTA et rollback se comportent comme sur l'appareil.

Entrées acceptées :

- **`.bin` d'application** (magic `0xE9`) : la flash est assemblée avec le bootloader embarqué et la table de partitions choisie dans le profil (stock X4, stock X3, CrossPoint) ;
- **dump flash complet** de la taille de la flash du profil, chargé tel quel.

Refus avant démarrage, avec message explicite :

- magic invalide ;
- format `.xota` chiffré ;
- `chip_id` de l'image incompatible avec le SoC du profil (ex. image S3 sur un X4) ;
- taille de flash déclarée supérieure à celle du profil.

**Persistance** : chaque appareil virtuel possède son fichier flash ; ce que le firmware écrit (NVS, OTA) survit aux redémarrages de l'émulateur. Commande de remise à zéro explicite.

### 5.2 Registres non émulés

Lecture : valeur de reset issue du SVD. Écriture : ignorée. Dans les deux cas, un avertissement **dédoublonné** donne le périphérique, le registre, l'adresse et le PC. Un mode strict (`--strict-mmio`) transforme ces accès en arrêt, pour la CI.

### 5.3 Affichage e-ink

- Les contrôleurs (SSD1677, UC8253…) sont des **interpréteurs de commandes** complets : plans RAM, fenêtres, pointeurs, LUT, déclenchement de rafraîchissement, bordure, deep sleep.
- Chaque pixel a un **état optique continu** (0 à 1). La LUT est appliquée trame par trame. Le flash du rafraîchissement complet, le ghosting des partiels, les niveaux de gris et la **durée de BUSY** en découlent.
- LUT internes (OTP) : le profil fournit des LUT de référence dérivées de la datasheet, signalées « approximation » dans l'UI.
- **Vue réaliste** : rendu optique, texture papier, animation. **Vue technique** : plans RAM bruts, zones de partiels surlignées, compteur de partiels depuis le dernier complet, durée de chaque update.
- Capture : **état logique** (exact au pixel, déterministe, utilisé en régression) et, en option, **état optique**.

### 5.4 Réseau et radio

- **Wi-Fi** : émulation du MAC radio au niveau des trames 802.11 (registres rétro-ingénierés, en s'appuyant sur le travail existant pour l'ESP32 classique). L'émulateur joue le point d'accès (SSID, WPA2, DHCP, DNS) et relaie l'IP vers Internet par une pile NAT en espace utilisateur, sans droits administrateur. En web, le transport passe par un relais WebSocket.
- **BLE** : appareils virtuels (tourne-page, téléphone) pilotés depuis l'UI ; passerelle vers l'adaptateur BLE de l'hôte, optionnelle et réservée au desktop.
- Ces deux blocs sont les plus risqués du projet : chacun commence par un **spike de faisabilité** dont la conclusion conditionne sa spec.

### 5.5 Horloge temps réel et capteurs

Les RTC (DS3231, BM8563) prennent l'heure de l'hôte, avec décalage ou heure figée configurables (indispensable pour des captures reproductibles). La jauge, l'IMU et la batterie sont pilotables depuis l'UI et depuis les scripts.

---

## 6. Outils

### 6.1 Moniteur de ressources

Le moniteur ne montre que des valeurs mesurées ; quand une valeur n'est pas mesurable, il l'indique au lieu d'afficher une estimation.

- **RAM** : occupation par région physique, plus haut niveau atteint, pile de chaque tâche. Le tas est affiché seulement si ses structures ESP-IDF sont reconnues.
- **CPU** : fréquence courante, charge (part du temps hors `WFI`), rapport de vitesse émulée/réelle.
- **Batterie** : niveau, USB branché ou non, décharge selon l'état d'alimentation, avec les consommations datasheet signalées comme telles.

### 6.2 Debug

Console série (UART et USB-Serial-JTAG), pause, pas-à-pas, registres, mémoire, points d'arrêt, **serveur GDB distant** (protocole RSP). Un ELF peut être associé au `.bin` pour obtenir les symboles.

### 6.3 Tactile (X4 Pro)

Souris ou trackpad vers doigts : clic = tap, glisser = swipe, modificateur + glisser = pinch à deux doigts. Les coordonnées sont converties dans le repère du capteur (rotation, résolution du GT911), avec la cadence de rapport et l'interruption réelles. Un overlay montre les touchers, les trajectoires, ce que le GT911 a transmis et si le firmware a lu le rapport.

### 6.4 CLI et tests automatisés

```
inkemu run --device x4 --firmware fw.bin --sd ./livres --script scenario.yaml
inkemu replay session.inkrec
inkemu gdb --device x4 --firmware fw.bin --port 3333
```

- Les scripts contiennent : boutons, gestes, attentes conditionnelles (écran stable, texte sur la console, délai virtuel), captures et comparaisons.
- Le code de sortie fait foi : 0 si tout passe, non nul sinon, avec rapport.
- Une session enregistrée (`.inkrec`) contient l'image, le profil et tous les événements.

### 6.5 Interface et langues

UI unique en TypeScript, empaquetée par Tauri sur desktop (Linux, macOS, Windows) et servie en statique sur le web. Toutes les chaînes passent par des fichiers de traduction FR/EN ; la langue système est détectée. Les secrets de signature et de notarisation passent par des variables d'environnement en CI, jamais par le dépôt.

---

## 7. Gestion des erreurs

Le principe est de ne jamais masquer une erreur.

| Situation | Comportement |
|---|---|
| Fichier d'entrée invalide | Refus avant démarrage, message expliquant la cause et la correction |
| Exception CPU, watchdog, panic du firmware | Comportement du vrai matériel (reset, dump) + entrée de journal avec PC et pile |
| Accès à un registre non émulé | Avertissement nommé (§5.2), arrêt en mode strict |
| Profil invalide | Refus au chargement avec la ligne fautive |
| Bug interne de l'émulateur (invariant violé) | Arrêt explicite avec rapport ; jamais d'état silencieusement faux |
| Hôte trop lent | Ralentissement du temps virtuel, jamais de cycles sautés |

---

## 8. Stratégie de test

| Niveau | Contenu |
|---|---|
| Unitaire | Chaque instruction, chaque commande de contrôleur, protocoles SD/I2C/SPI, parsing d'images, validation de profils |
| Conformité | `riscv-arch-test` (puis équivalent Xtensa) |
| Différentiel | Lockstep contre QEMU sur des firmwares réels |
| Intégration | Firmwares de référence (CrossPoint, Papyrix, firmware d'exemple X4) démarrés jusqu'à leur écran d'accueil ; comparaison à des captures validées |
| Timing | Tolérance CoreMark (§4.2) |
| Déterminisme | Deux exécutions de la même session produisent la même empreinte de trace |

Les firmwares de test qu'on doit compiler (CoreMark, micro-benchmarks) sont construits en Docker avec une version ESP-IDF épinglée, pour que la CI soit reproductible.

---

## 9. Feuille de route

| # | Sous-projet | Livrable utilisable |
|---|---|---|
| 1 | Cœur RV32IMC + ESP32-C3 minimal + CLI | Un firmware X4 démarre et affiche ses logs en console |
| 2 | Périphériques X4 + UI desktop + rendu e-ink | **Premier jalon visuel** : le X4 virtuel affiche et réagit aux boutons |
| 3 | X3 | UC8253, I2C, BQ27220, DS3231, QMI8658 |
| 4 | Debug, moniteur de ressources, mode headless complet | GDB, moniteur, scripts, régression visuelle |
| 5a | Spike puis Wi-Fi | Le firmware se connecte et accède à Internet |
| 5b | BLE | Appareils virtuels + passerelle hôte |
| 6 | Cible WASM + UI web + relais réseau | Version navigateur |
| 7 | Xtensa LX7 + ESP32-S3 | X4 Pro (tactile, PSRAM, SDMMC, BM8563, éclairage) et X4C |
| 8 | Banc de calibration du timing | Modèles calibrés sur matériel |
| 9 | Distribution | Paquets Linux, `.dmg` signé et notarisé, `.msi`, publication web |

---

## 10. Sous-projet 1 — spécification détaillée

### 10.1 But

Exécuter un `.bin` ESP32-C3 réel depuis la CLI, de la ROM jusqu'à l'application, avec ses logs sur la console. Toutes les fondations (contrats, déterminisme, timing, validation) doivent être en place.

### 10.2 Contenu

1. **Vérification des licences** de la ROM et des SVD Espressif, et décision embarqué/téléchargé consignée dans un ADR. Cette étape conditionne la suite.
2. Workspace Cargo (`rust-toolchain.toml` épinglé sur 1.99.0), CI GitHub Actions (fmt, clippy, tests, `cargo deny` pour les licences).
3. `emu-core` : contrats, `Scheduler`, `TimingModel`, `EventLog`.
4. `svd-regs` : génération depuis le SVD ESP32-C3.
5. `cpu-rv32` : RV32IMC + Zicsr, CSR et mécanisme d'interruption spécifiques au C3, cache de décodage.
6. `firmware` : parsing d'image, refus des formats invalides, assemblage de la flash avec la table de partitions.
7. `soc-esp32c3`, avec ces périphériques :
   - ROM ;
   - cache et MMU XIP ;
   - contrôleur flash SPI1 ;
   - horloges et PLL ;
   - eFuse ;
   - matrice d'interruptions ;
   - SYSTIMER ;
   - TIMG avec les watchdogs actifs ;
   - RTC_CNTL (reset, deep sleep, réveil par timer) ;
   - GPIO ;
   - UART0 ;
   - USB-Serial-JTAG.
8. `boards` : chargement et validation des profils TOML ; profils `x4` et `x3` complets (même si leurs puces externes arrivent aux sous-projets 2 et 3).
9. `emu-cli` : `inkemu run` (console sur stdout, `--max-cycles`, `--strict-mmio`), `inkemu inspect` (affiche l'en-tête et les segments d'une image), enregistrement et rejeu.
10. Harnais lockstep QEMU (outil de développement, hors binaire distribué).
11. Pipeline Docker reproductible pour les firmwares de test (CoreMark, micro-tests).

### 10.3 Critères d'acceptation

1. `riscv-arch-test` RV32IMC + Zicsr : 100 % de réussite.
2. `inkemu run --device x4 --firmware <CrossPoint x3-x4>.bin` : la ROM, le bootloader et l'application s'exécutent, et les logs de boot du firmware apparaissent sur la sortie standard. Les premiers accès aux périphériques des sous-projets suivants (GP-SPI2, SAR-ADC…) apparaissent comme avertissements nommés, pas comme plantage muet.
3. Lockstep contre QEMU sur ce même boot : aucune divergence sur les 50 premiers millions d'instructions, ou jusqu'au premier accès à un périphérique que QEMU n'émule pas.
4. Un firmware de test qui bloque déclenche un reset par watchdog après le délai configuré par le firmware.
5. CoreMark dans ±15 % du score publié.
6. Deux exécutions de la même session produisent la même empreinte de trace.
7. Chaque cas de refus du §5.1 est couvert par un test qui vérifie le message.
8. Couverture de tests ≥ 80 % sur les crates du sous-projet ; 100 % sur le décodage d'instructions et le parsing d'images.

### 10.4 Risques

| Risque | Atténuation |
|---|---|
| Licence ROM non redistribuable | Téléchargement au premier lancement avec empreinte épinglée |
| Écarts entre la documentation et le comportement réel du C3 (CSR, interruptions) | Le lockstep QEMU sert d'arbitre ; les écarts sont documentés dans le code avec leur source |
| Firmware qui attend un périphérique non émulé en boucle infinie | Avertissement nommé + watchdog ; c'est le signal attendu qui ouvre le sous-projet 2 |
| Performance de l'interpréteur insuffisante | Benchmarks dès SP1 ; saut des périodes d'attente ; le temps virtuel reste exact même en cas de ralentissement |
