# SP2a — Les puces externes du X4 : design

Date : 2026-10-15. Spec globale : `docs/superpowers/specs/2026-10-11-inkemu-design.md` §2 (« polarité de BUSY et
fréquence SPI … paramètres de profil »), §3.1 (machine déterministe), §3.2 (crate `chips` : « les puces externes ne
connaissent pas le SoC »), §3.3 (valeurs ADC des échelles dans le profil), §5.3 (affichage e-ink), §8 (« chaque
commande de contrôleur » en test unitaire), §9 point 2. Recherche : `docs/research/sp2a-external-chips.md` (RC
ci-dessous) et `docs/research/xteink-hardware.md`. Contrats repris : ADR 0003 (temps de l'appareil en cycles d'une
base fixe ; un modèle est appelé à l'instant qu'il demande, jamais interrogé).

Découpage de SP2 : SP2a (ce document) les puces externes ; SP2b les périphériques du SoC et les traits de signaux ;
SP2c l'application de bureau ; SP2d le câblage du X4 dans `boards` et les adaptateurs entre les signaux et les
interfaces décrites ici.

## 1. But

Une crate `crates/chips`, qui ne dépend que d'`emu-core`, ne connaît pas le SoC, compile pour `wasm32` et ne fait
aucune E/S hôte. Elle livre :

1. le contrôleur **SSD1677**, interpréteur complet de ses commandes, exposé au niveau des commandes (écrire une
   commande, écrire ou lire une donnée, ligne de reset, BUSY), qui produit une image du panneau physique après chaque
   rafraîchissement ;
2. l'**échelle de boutons** lue par l'ADC ;
3. la **batterie** lue par l'ADC.

## 2. Décisions

| # | Décision | Raison |
|---|---|---|
| A1 | **Interface au niveau des commandes**, temps passé à chaque appel : `write_command(now, octet)`, `write_data(now, octet)`, `read_data(now) -> Option<u8>`, `set_reset(now, actif)`, `advance(now)`, `next_deadline()`, `is_busy(now)`, `busy_pin(now)`. `now` est un instant en cycles de la base de temps de la machine ; la fréquence de cette base est donnée à la construction (`Frequency`). Un instant antérieur au dernier vu vaut le dernier : le temps de la puce ne recule pas. | Le découpage en octets SPI, D/C# et CS# appartient à SP2d ; ADR 0003 pour le temps. |
| A2 | **La puce est appelée, jamais interrogée** : `next_deadline()` donne le prochain instant où BUSY change ou où une image est publiée ; SP2d y planifie un appel à `advance`. Chaque appel commence par traiter ce qui est échu. | ADR 0003, point 3 ; FreeInk attend BUSY par interruption sur front (RC §2) : le front doit tomber à un instant connu. |
| A3 | **BUSY se compte en temps d'appareil**, jamais en temps hôte : chaque opération a une durée en nanosecondes, convertie en cycles par `Frequency::nanos_to_cycles` (arrondi vers le haut). `is_busy` dit l'état logique (occupé) ; `busy_pin` le niveau de la broche selon `busy_active_high` (vrai par défaut, RC finding 4). | Spec §5.3, §2. |
| A4 | **Toutes les commandes de la fiche technique ont un traitement nommé.** Modélisées : `01 03 04 0C 10 11 12 14 15 18 1A 1B 20 21 22 24 26 27 28 29 2B 2C 2D 2E 2F 32 37 38 3C 41 44 45 46 47 4E 4F 7F`. Décrites trop brièvement pour un effet vérifiable (programmation et chargement d'OTP, CRC, code initial, capteur externe, tramage : `08 09 0A 1C 25 2A 30 31 34 35 36 39 4D`) : diagnostic `UnsupportedCommand`, données acceptées sans effet. Réservées (`3A 3B`) : `ReservedCommand`. Toute autre : `UnknownCommand`, données ignorées. | Spec §5.3 « interpréteurs de commandes complets » et §7 « ne jamais masquer une erreur » ; RC §3.6. |
| A5 | **Diagnostics nommés** (`Diagnostic { at, kind }`), relevés par `take_diagnostics`, file bornée à 1 024 entrées avec un compteur de ce qui a été perdu. Un diagnostic par commande fautive, pas par octet de données. | Mémoire bornée quoi que fasse le firmware. |
| A6 | **Commande reçue pendant BUSY** : ignorée avec ses données et rapportée (`WhileBusy`), sauf `2F` (lecture d'état), exécutée. En sommeil profond, tout est ignoré et rapporté (`WhileAsleep`) jusqu'au reset matériel ; RES# bas, tout est ignoré (`WhileInReset`). | La fiche interdit d'envoyer une commande pendant BUSY sans dire l'effet ; un bit « busy » d'état n'a de sens que s'il se lit pendant BUSY (RC §5). |
| A7 | **Mémoire** : deux plans de 960 × 680 bits ; adresse X en pixels, un octet = 8 pixels, poids fort à gauche ; fenêtres, compteurs et les quatre sens de saisie, compteur en X (AM = 0) ou en Y (AM = 1), retour au début de la fenêtre au-delà de sa fin. Un compteur hors de la mémoire écrit dans le vide (diagnostic) ; un compteur hors de la fenêtre au début d'une écriture est rapporté. L'écriture automatique (`46` rouge, `47` noir et blanc, selon la fiche) remplit toute la mémoire en damier selon ses pas. | RC §3.1. |
| A8 | **La séquence d'affichage** : `22` choisit les étapes (horloge, analogique, température, LUT, mode 2, affichage, coupure analogique, coupure de l'horloge, un bit chacune) et `20` les exécute dans cet ordre. Une étape qui demande l'horloge ou l'analogique alors qu'ils sont coupés est sautée et rapportée. Le chargement de LUT cherche dans l'OTP du profil la dernière forme d'onde du mode demandé dont la plage contient la température (borne basse exclue) ; sans résultat, l'affichage n'a pas lieu (diagnostic). | RC §3.3, §3.5. |
| A9 | **L'OTP est un paramètre** : une liste de formes d'onde (mode, plage de température, 112 octets). Le profil GDEQ0426T82 en fournit trois de référence (RC §5), construites pour reproduire les durées mesurées et les couleurs attendues, signalées « approximation » (spec §5.3). | Aucune source ne publie l'OTP du panneau. |
| A10 | **Modèle optique** : chaque pixel a un état continu entre noir et blanc (entier, déterministe) ; chaque trame d'une phase le déplace proportionnellement à (tension de source − excursion de VCOM autour du DCVCOM, donnée par LUT4) × durée de trame, borné au noir et au blanc ; le DCVCOM lui-même ne pilote rien (hypothèse : il compense le retour de charge des transistors) ; une tension positive fonce le pixel (paramètre `positive_drives_black`, vrai pour le X4). Une forme d'onde entière se réduit à une translation bornée par entrée de LUT, puis à une évaluation par pixel. L'amplitude d'un passage du noir au blanc est un paramètre (6 000 V·ms par défaut). | Spec §5.3 « état optique continu », « la LUT est appliquée trame par trame » ; RC §6 : la LUT de gris du X4 ne garde ses pixels de code 00 qu'avec des phases VSS, ce qu'un DCVCOM pilotant contredirait. Le gris, le flash du complet et la rémanence des partiels en découlent. |
| A11 | **Image du panneau** (`PanelImage`) : largeur, hauteur, pixels en niveaux de gris (0 noir, 255 blanc), bordure, numéro de génération, type (`PowerOn`, `Full` pour le mode d'affichage 1, `Partial` pour le mode 2), instant, interrompue ou non. Publiée à la **fin de l'étape d'affichage**, avant la fin de BUSY si une coupure suit. Un reset matériel pendant l'affichage publie l'état atteint à cet instant (`interrupted`). La génération 0 est l'état à la mise sous tension. | Mission ; un firmware qui coupe l'analogique après l'affichage ne change plus la dalle. |
| A12 | **Géométrie paramétrée** : largeur et hauteur du panneau (800 × 480 par défaut), première source, colonnes inversées, réglage de balayage des grilles qui donne un ordre séquentiel sur ce panneau (SM, GD) et lignes inversées. Le X4 : SM = 1, lignes inversées (RC §4 : « Gates are physically reversed on this panel »). Le multiplexage (`01`) choisit les grilles balayées ; les lignes au-delà ne changent pas. | Spec §5.3 ; mission « la géométrie est un paramètre ». |
| A13 | **Durées et valeurs incertaines = paramètres**, valeurs par défaut sourcées ou marquées hypothèse (RC §5) : période de trame 20 ms par code, analogique 100 ms à l'allumage et 200 ms à la coupure, température interne 5 ms, LUT 1 ms, reset logiciel 2 ms, reset matériel 1 ms, écriture automatique 2 ms, détection VCI 1 ms. Température ambiante 25 °C, modifiable pendant l'exécution. Aucun capteur externe. VCI 3,3 V. | Spec §2. |
| A14 | **Échelle de boutons** : un niveau au repos et un niveau par bouton, **en millivolts à la broche**, paramétrés par le profil ; plusieurs boutons appuyés : le niveau le plus bas l'emporte (échelle en série, hypothèse). La conversion en code est celle de l'ADC (SP2b). | La tension est la grandeur physique ; le code dépend de l'atténuation que choisit le firmware. RC §7. |
| A15 | **Batterie** : une tension de cellule, réglable, et un pont diviseur (deux résistances) ; la broche voit cellule × R_bas / (R_haut + R_bas), arrondi au millivolt. | RC §8 ; mission « une tension paramétrée ». |
| A16 | **Aucune E/S hôte** : `crates/chips/clippy.toml` est une copie de l'interdiction des autres crates, tenue égale par `crates/boards/tests/host_io_ban.rs` (sept copies) ; `chips` rejoint l'étape WebAssembly de la CI. | Règles du dépôt. |
| A17 | **`0x1A` prend un ou deux octets** sans diagnostic ; avec un seul, `A[3:0]` garde sa valeur. `0x21` prend de même un second octet, conservé sans effet. | Le demi-rafraîchissement du X4 envoie `1A 5A` seul (RC §4) et fonctionne sur l'appareil : un diagnostic à chaque page serait du bruit, pas une erreur du firmware. |

## 3. Interface publique (pour SP2d)

```rust
// crates/chips/src/lib.rs
pub use analog::{Battery, BatteryError, ButtonLadder, LadderButton, LadderError, Millivolts};
pub use ssd1677::{
    ConfigError, Diagnostic, DiagnosticKind, DisplayMode, GateScan, Optics, OtpWaveform, PanelGeometry,
    PanelImage, Plane, RefreshKind, Ssd1677, Ssd1677Config, Ssd1677Timing, Stage, Temperature,
};
```

Le détail des signatures est dans la note de passation (§ « Interface publique »), tenu à jour avec le code.

## 4. Hors périmètre

- Les lignes SPI, D/C#, CS#, BUSY et RES# et leur câblage (SP2d), l'ADC et sa conversion (SP2b), le rendu (SP2c).
- L'OTP réel du GDEQ0426T82, la calibration du modèle optique et des durées (sous-projet 8 : un appareil).
- La persistance de l'image du panneau d'une exécution à l'autre, l'animation trame par trame (l'interface le
  permet : l'état à un instant de l'affichage se calcule déjà pour une interruption).
- UC8253 et les autres contrôleurs (sous-projet 3).

## 5. Critères d'acceptation

1. Un test unitaire par commande et par mode (sens de saisie, AM, options de `21`, étapes de `22`, modes 1 et 2,
   bordure, sommeil profond, resets), plus les cas limites (fenêtre au bord de la mémoire, commande pendant BUSY,
   reset pendant l'affichage, données sans commande, paramètres manquants ou en trop, valeurs réservées).
2. Les séquences exactes du pilote FreeInk du X4 (initialisation, complet, rapide, demi, gris d'usine, coupure,
   sommeil) produisent l'image attendue, avec la durée de BUSY attendue.
3. `cargo fmt`, `tools/ci/clippy.sh -p chips` (`-D warnings`), `cargo doc -p chips` (`-D warnings`),
   `cargo test -p chips`, `cargo check --target wasm32-unknown-unknown -p chips` passent ; `host_io_ban` passe.
