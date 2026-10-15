# SP2a — Note de passation

Plan : `docs/superpowers/plans/2026-10-15-sp2a-chips.md` (2a-R1 à 2a-R23). Spec :
`docs/superpowers/specs/2026-10-15-sp2a-chips-design.md` (A1–A17). Recherche : `docs/research/sp2a-external-chips.md`.
Branche partie de `main` à `194f286`.

## Résultat

Une crate `crates/chips` (dépend d'`emu-core` et de `thiserror`, aucune E/S hôte, compile pour `wasm32`) :

- **SSD1677** : interpréteur de toutes les commandes de la table 7-1 de la fiche technique Rev 1.0, plans de RAM
  noir et blanc et rouge, fenêtres, compteurs et les huit modes de saisie, LUT et voltages, séquence `22`/`20` étape
  par étape, OTP paramétrée (trois formes d'onde de référence pour le GDEQ0426T82, approximations), bordure, sommeil
  profond, resets logiciel et matériel, lectures, BUSY en temps d'appareil, diagnostics nommés et bornés ; modèle
  optique entier qui produit une `PanelImage` en niveaux de gris après chaque affichage.
- **Échelle de boutons** et **batterie** lues par l'ADC, en millivolts à la broche.

Vérifié sur le poste de développement (partagé, `CARGO_BUILD_JOBS=3`) : `cargo fmt --all --check` ;
`tools/ci/clippy.sh -p chips --all-targets --all-features -- -D warnings` ; `cargo doc -p chips` avec
`RUSTDOCFLAGS=-D warnings` ; `cargo test -p chips` (79 tests unitaires, 8 d'intégration) ;
`cargo check --target wasm32-unknown-unknown -p chips` ; `cargo test -p boards --test host_io_ban` (3) ;
`cargo test -p inkemu-lockstep --test readme` (2). **Non lancés**, laissés à la CI : la suite complète du workspace,
clippy et rustdoc sur tout le workspace, la couverture.

Ce que reproduisent les tests sur la dalle 800×480, avec les séquences exactes de `FreeInk` (pilote du X4 dans
`CrossPoint` 1.6.5) et les durées par défaut :

| Séquence | BUSY | Image |
|---|---|---|
| `begin` (reset, `12`, `46 F7`, `47 F7`) | 44 ms de bout en bout | RAM blanche |
| Complet `22 F7`, analogique coupé avant | 1 906 ms (image publiée à 1 706 ms) | exacte, dans le bon sens |
| Rapide `22 FC`, analogique coupé / allumé | 606 / 506 ms | pixels changés à 95 % d'une excursion (13 et 242), les autres intacts |
| Demi `1A 5A` (un octet) `22 D7`, allumé | 1 701 ms | forme d'onde au-dessus de 50 °C |
| Gris d'usine (`32`…, `22 CC`), coupé | 1 100 ms | niveaux 255, 153, 128, 0 |
| Sommeil `10 03` | BUSY haut, aucune échéance | la dalle garde sa page |

## Interface publique (pour SP2d)

```rust
// Temps : Cycles de la base de temps de la machine (emu_core), fréquence donnée à la construction.
use chips::{
    Battery, BatteryError, ButtonLadder, LadderButton, LadderError, Millivolts,
    ConfigError, Diagnostic, DiagnosticKind, DisplayMode, GateScan, Optics, OtpWaveform, PanelGeometry,
    PanelImage, Plane, RefreshKind, Ssd1677, Ssd1677Config, Ssd1677Timing, Stage, Temperature,
};
// Aussi : chips::ssd1677::{RAM_SOURCES (960), RAM_GATES (680), LUT_BYTES (112), MAX_DIAGNOSTICS (1024)},
// chips::ssd1677::reference::gdeq0426t82_otp() -> Vec<OtpWaveform>.

impl Ssd1677 {
    pub fn new(config: Ssd1677Config, time_base: Frequency) -> Result<Ssd1677, ConfigError>;
    pub fn write_command(&mut self, now: Cycles, command: u8);   // octet avec D/C# bas
    pub fn write_data(&mut self, now: Cycles, data: u8);         // octet avec D/C# haut, écrit
    pub fn read_data(&mut self, now: Cycles) -> Option<u8>;      // lu ; None = la ligne flotte
    pub fn set_reset(&mut self, now: Cycles, asserted: bool);    // asserted = RES# bas
    pub fn advance(&mut self, now: Cycles);                      // à appeler à next_deadline()
    pub fn next_deadline(&self) -> Option<Cycles>;               // BUSY tombe ou image publiée
    pub fn is_busy(&self, now: Cycles) -> bool;                  // état logique
    pub fn busy_pin(&self, now: Cycles) -> bool;                 // niveau de la broche (polarité de la config)
    pub fn panel_image(&self) -> &PanelImage;                    // dernière image publiée
    pub fn ram(&self, plane: Plane) -> &[u8];                    // 680 lignes de 120 octets
    pub fn take_diagnostics(&mut self) -> Vec<Diagnostic>;
    pub fn dropped_diagnostics(&self) -> u64;
    pub fn set_ambient(&mut self, ambient: Temperature);         // capteur interne
    pub fn config(&self) -> &Ssd1677Config;
}

pub struct PanelImage {
    pub width: u16, pub height: u16,
    pub pixels: Vec<u8>,      // ligne par ligne depuis le coin haut-gauche ; 0 noir, 255 blanc
    pub border: u8,
    pub generation: u64,      // 0 à la mise sous tension, +1 par affichage publié
    pub kind: RefreshKind,    // PowerOn | Full (mode 1) | Partial (mode 2)
    pub interrupted: bool,    // reset matériel pendant l'affichage
    pub at: Cycles,
}

pub struct Ssd1677Config {
    pub panel: PanelGeometry,            // gdeq0426t82() : 800×480, SM = 1, lignes inversées
    pub optics: Optics,                  // full_swing (mV·ns), positive_drives_black, initial_gray
    pub timing: Ssd1677Timing,           // durées en ns, frame_period: [u64; 16]
    pub otp: Vec<OtpWaveform>,
    pub busy_active_high: bool,          // true sur le X4
    pub ambient: Temperature,            // 25 °C
    pub external_sensor: Option<Temperature>, // None sur le X4
    pub vci_millivolts: u32,             // 3 300
    pub user_id: [u8; 10],
}
// Ssd1677Config::gdeq0426t82() == Ssd1677Config::default().

impl ButtonLadder {
    pub fn new(idle: Millivolts, buttons: Vec<LadderButton>) -> Result<ButtonLadder, LadderError>;
    pub fn idle(&self) -> Millivolts;
    pub fn buttons(&self) -> &[LadderButton];            // LadderButton { name: String, level: Millivolts }
    pub fn index_of(&self, name: &str) -> Option<usize>;
    pub fn set_pressed(&mut self, index: usize, pressed: bool) -> Result<(), LadderError>;
    pub fn is_pressed(&self, index: usize) -> Option<bool>;
    pub fn release_all(&mut self);
    pub fn voltage(&self) -> Millivolts;                 // le plus bas des niveaux appuyés, sinon idle
}

impl Battery {
    pub fn new(cell: Millivolts, top_ohms: u32, bottom_ohms: u32) -> Result<Battery, BatteryError>;
    pub fn cell(&self) -> Millivolts;
    pub fn set_cell(&mut self, cell: Millivolts);
    pub fn voltage(&self) -> Millivolts;                 // cellule × bas / (haut + bas), arrondi
}
```

Usage attendu dans SP2d :

1. **SPI** : l'adaptateur de GP-SPI2 assemble les octets quand CS (GPIO21) est bas et passe chacun à
   `write_command` ou `write_data` selon D/C (GPIO4), à l'instant de son dernier bit. La carte SD partage SCLK et MOSI
   (CS GPIO12) : un octet émis avec CS de l'écran haut ne va pas au SSD1677. La lecture demi-duplex (sonde du
   firmware : MOSI repassé en entrée) appelle `read_data` ; `None` laisse le niveau au tirage de la broche.
2. **BUSY** (GPIO6) : après chaque appel, poser l'entrée à `busy_pin(now)` ; planifier un événement à
   `next_deadline()` qui appelle `advance` puis remet le niveau à jour (et lève l'interruption GPIO sur front si
   le firmware l'a armée : FreeInk attend BUSY par interruption `CHANGE`).
3. **RES#** (GPIO5) : `set_reset(now, niveau_bas)` à chaque changement de sortie.
4. **Image** : comparer `panel_image().generation` après chaque `advance` ou appel pour pousser une nouvelle
   image à l'interface (SP2c) et aux captures de régression.
5. **ADC** : GPIO1 et GPIO2 lisent `voltage()` de deux `ButtonLadder`, GPIO0 celle d'une `Battery` (10 kΩ / 10 kΩ) ;
   la conversion en code selon l'atténuation est celle de SP2b. Les appuis et la tension de la cellule sont des
   entrées d'une exécution, à journaliser pour l'enregistrement et le rejeu (ADR 0007).
6. **Profil X4** : niveaux proposés (recherche §7, droite nominale 0–2 500 mV à 12 dB, à reprendre si SP2b choisit
   une autre fonction de transfert) : GPIO1 repos 3 300 mV, back 2 144, confirm 1 645, left 911, right 3 ;
   GPIO2 repos 3 300, up 1 369, down 3. Batterie : cellule 3 700 mV par défaut (hypothèse), diviseur 10 k/10 k.
   Power est un GPIO numérique (GPIO3, actif bas), pas une échelle.

## Décisions prises sans l'utilisateur

- **Le DCVCOM ne pilote aucun pixel** (A10, 2a-R12) : décidé en écrivant les tests, quand la première version
  faisait bouger tous les pixels pendant les phases VSS, ce que contredit la LUT de gris du X4.
- **`0x1A` accepte un seul octet** (A17) : le demi-rafraîchissement du X4 n'en envoie qu'un.
- **`0x46` remplit le plan rouge**, comme la fiche, contre le nom que lui donnent FreeInk et Papyrix (2a-R9).
- **Une commande pendant BUSY est ignorée**, sauf la lecture d'état (A6).
- **Formes d'onde OTP de référence** construites pour le modèle (A9), avec le bloc de tensions de la LUT d'usine.
- **Les niveaux des échelles sont des tensions**, pas des codes (A14).

## Laissé aux héritiers

| Sujet | Héritier |
|---|---|
| Adaptateur SPI/DC/CS/BUSY/RES vers `Ssd1677`, ADC vers `ButtonLadder`/`Battery`, profil X4 (`boards`), journal des entrées (appuis, cellule, température) | SP2d |
| Conversion tension → code de la SAR ADC, et la fonction de transfert qui fixe les niveaux en millivolts du profil | SP2b |
| Afficher `PanelImage` (vue réaliste), la RAM brute (`ram`, vue technique), le compteur de partiels depuis le dernier complet (`kind`), le statut « approximation » des formes d'onde et du modèle optique | SP2c |
| Animation du rafraîchissement trame par trame : `Waveform::transfer` sait déjà s'arrêter à un instant ; il faut l'exposer | SP2c / SP4 |
| Persistance de l'image de la dalle d'une exécution à l'autre (le X4 démarre sur l'écran de veille) | SP2d / SP4 |
| OTP réelle du GDEQ0426T82, période de chaque code de trame, amplitude d'une excursion, durées des étapes : mesurer sur un appareil | Sous-projet 8 |
| UC8253 / UC8279 (X3), UC8179 (X4 Pro) | Sous-projets 3 et 7 |

## Limites de preuve

- Les séquences du firmware sont prises au code de FreeInk (celui de `CrossPoint` 1.6.5), pas observées sur un
  appareil ni sur un firmware tournant sous inkemu (il faut SP2b et SP2d).
- Les durées, l'OTP, l'amplitude optique, le sens de saisie en X décrémenté, l'état de la RAM au démarrage et après
  sommeil sont des hypothèses nommées (recherche §5) ; les tests épinglent le comportement du modèle, pas celui de
  la puce.
- Le firmware d'origine du X4 n'a pas été lu (binaire seulement) ; CrossPoint et Papyrix l'ont été.

## Cartographie

Ajouts à reporter dans `~/.claude/projects/-home-rony-projects-xteink-emulator/memory/` (non modifiés ici : trois
agents travaillent en parallèle) :

- **`architecture_overview.md`** : nouvelle crate `chips` (SP2a), dépend d'`emu-core` et `thiserror` seulement, ne
  connaît pas le SoC ; contrat : octets de commande et de données datés en `Cycles` de la base de temps, puce
  rappelée à `next_deadline()` (ADR 0003), image du panneau publiée à la fin de l'affichage ; sources analogiques
  en millivolts à la broche. Interdiction des E/S hôte : sept copies de `clippy.toml` (ajout de `chips`).
- **`backend_models.md`** : types publics de `chips` — `Millivolts`, `LadderButton`, `ButtonLadder`,
  `LadderError`, `Battery`, `BatteryError` ; `Ssd1677`, `Ssd1677Config`, `PanelGeometry`, `GateScan`, `Optics`,
  `Ssd1677Timing`, `Temperature`, `DisplayMode`, `OtpWaveform`, `ConfigError`, `PanelImage`, `RefreshKind`,
  `Plane`, `Diagnostic`, `DiagnosticKind` (19 variantes), `Stage` ; constantes `RAM_SOURCES`, `RAM_GATES`,
  `LUT_BYTES`, `MAX_DIAGNOSTICS`.
- **`backend_services.md`** : modules de `chips` — `analog.rs` ; `ssd1677/mod.rs` (table des commandes, machine à
  états, BUSY, resets, lectures, activation `22`/`20`), `config.rs` (paramètres, recherche de forme d'onde OTP,
  validation), `ram.rs` (plans, fenêtre, compteur, écriture automatique), `waveform.rs` (tables de tensions, LUT,
  translations bornées), `panel.rs` (grilles → lignes, état optique, `PanelImage`), `reference.rs` (OTP de
  référence du GDEQ0426T82), `diagnostic.rs`.
- **`api_routes.md`** : tests de `chips` — 79 unitaires (`src/**`), 8 d'intégration (`tests/x4_driver.rs`,
  séquences FreeInk) ; `host_io_ban` couvre sept crates ; l'étape « Build the model for WebAssembly » de la CI
  inclut `-p chips`.
- **`frontend_spa.md`** : ce que SP2a laisse à SP2c — `PanelImage` (niveaux de gris, bordure, génération, type,
  interrompue), `ram()` pour la vue technique, compteur de partiels à tenir côté interface, statut « approximation »
  de l'OTP de référence et du modèle optique, température ambiante réglable (`set_ambient`).
- **`frontend_nextjs.md`** : `chips` compile pour `wasm32-unknown-unknown`.
- **`integrations.md`** : fiche technique SSD1677 Rev 1.0 (Good Display, SHA-256
  `4b5cc241f410722e54c4d756424907b90ed988502216ed5ed864dd0df78b7e0c`, non stockée) ; FreeInk SDK
  `7fcd3b34c108f28c086ade3b433924022dbc0998` (sous-module de CrossPoint `bb855ef9`), Papyrix `ee071b17` ; aucune
  dépendance nouvelle.
- **`database_schema.md`** : aucun état persistant ajouté ; l'image de la dalle n'est pas sauvegardée (laissé à
  SP2d/SP4).
- **`tech_debt.md`** : hypothèses de SP2a (recherche §5) et reports du tableau « Laissé aux héritiers » ci-dessus.
