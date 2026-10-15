# SP2d — Le câblage du X4 : design

Date : 2026-10-15. Point de départ : `main` à `4b02a91` (SP2a et SP2c fusionnés) ; SP2b (PR #18) n'est pas fusionné,
et ce document s'appuie sur l'état de sa branche (§9 : ce qui dépend de sa version finale). Spec globale :
`docs/superpowers/specs/2026-10-11-inkemu-design.md` §2 (matériel), §3.1 (machine déterministe, entrées horodatées),
§3.2 (« seul `boards` relie SoC et puces »), §3.3 (le câblage et les valeurs ADC sont dans le profil), §5.3 (rendu
e-ink), tableau des sous-projets (SP2 : « le X4 virtuel affiche et réagit aux boutons »). Recherche :
`docs/research/sp2d-x4-wiring.md` (RW ci-dessous), qui s'appuie sur `xteink-hardware.md`, `sp2a-external-chips.md`
et `esp32c3-deep-sleep.md` §5. Contrats repris : ADR 0003 (temps de l'appareil), 0004 et 0006 (entrées d'une
exécution), 0007 (sessions), 0011 (crate `host`), 0012 (signaux, SP2b).

Découpage de SP2 : SP2a les puces ; SP2b les périphériques du SoC et les traits de signaux ; SP2c l'application ;
**SP2d (ce document) le câblage du X4** ; SP2e, nouveau, la carte SD (§8).

## 1. But

À la fin de SP2d, dans l'application de bureau, le X4 virtuel s'allume par son bouton d'allumage, affiche ce que
son firmware dessine sur le SSD1677, réagit à ses sept boutons, s'endort et se réveille comme l'appareil, et une
session enregistrée avec des appuis se rejoue à l'identique. Concrètement :

1. le SSD1677 de SP2a et les échelles de boutons, la batterie et le bouton d'allumage sont branchés aux broches du
   X4 par les traits de SP2b, selon le profil ;
2. un appui de l'utilisateur est une **entrée de la carte**, datée en temps d'appareil, tenue dans le journal de la
   machine et dans les sessions (format 2) ;
3. le SoC sait se réveiller d'un sommeil profond par GPIO, maintenir une broche (`PAD_HOLD`) et s'éteindre quand la
   carte coupe son alimentation ;
4. l'interface montre l'écran réel à la place de la mire, avec la mention « rendu approché » tant que l'optique du
   panneau n'est pas mesurée ;
5. un firmware de référence à nous prouve en CI, pixel par pixel, le chemin firmware → SPI → SSD1677 → image, et
   le chemin bouton → ADC/GPIO → firmware ; CrossPoint 1.6.5 est essayé en local.

**Ce que SP2d ne donne pas : l'écran d'accueil de CrossPoint.** CrossPoint monte la carte SD avant tout autre
écran ; sans carte, il affiche « SD card error » et s'arrête là (RW §1 point 10). L'écran d'accueil demande un
modèle de carte SD en SPI et un stockage hôte, qui touchent aussi aux sessions et à l'interface : c'est SP2e (§8).
SP2d mène CrossPoint jusqu'à son écran « SD card error », droit et lisible, qui suffit à trancher la rotation.

## 2. Décisions

| # | Décision | Raison |
|---|---|---|
| 2d-D1 | **Où vit le code.** `chips` implémente les traits neutres de `emu-core::signals` pour ses puces (adaptateur du SSD1677 aux lignes, `AnalogSource` des échelles et de la batterie, nouveau bouton poussoir), sans connaître le SoC ; `boards` lit le câblage du profil et appelle `attach_*` : il est le seul à relier broches et puces ; `soc-esp32c3` reçoit les entrées de la carte, le réveil GPIO, le maintien des broches et la coupure d'alimentation ; `host` fait de la carte câblée les ports `PanelSource` et `ButtonPort` ; `session` passe au format 2 ; `app` affiche. | Spec globale §3.2 : les puces ne voient que des signaux, seul `boards` relie. Un adaptateur qui ne voit que des signaux est du comportement de puce, testable sans SoC. |
| 2d-D2 | **Une base de temps d'un gigahertz pour le SSD1677 câblé** : `Ssd1677::new(config, 1 GHz)`, si bien qu'un instant `Nanos` des signaux est un `Cycles` de la puce, sans conversion ni arrondi. | Les traits de SP2b datent en nanosecondes ; la puce de SP2a compte en cycles d'une base donnée à la construction (2a-R4). |
| 2d-D3 | **Adaptateur SPI** : `select(true)` ouvre une sélection, `select(false)` la ferme ; une sélection sans transfert ne fait rien (2b-R6, l'impulsion de `spicommon_cs_initialize`) ; chaque octet d'un transfert est une commande si D/C est bas, une donnée s'il est haut ; la MISO n'est pas pilotée (le panneau du X4 n'a pas de sortie sur le bus) ; un transfert sans D/C est signalé par un diagnostic de câblage et ignoré. Une commande et ses données peuvent partager une sélection ou non : le SSD1677 ne dépend pas de CS. | `EpdBus.cpp:148-184` : CS et D/C écrits par logiciel autour de chaque transfert, D/C qui change dans une sélection (RW §1 point 2). |
| 2d-D4 | **Le bus en bit-bang** : le SSD1677 écoute SCLK, MOSI, D/C et CS en `GpioOutput`. CS bas, chaque front montant de SCLK lit le niveau de MOSI, MSB d'abord ; le huitième fait une commande ou une donnée selon D/C. Après une commande de lecture, D/C haut, la puce **pilote MOSI** (`GpioInput` sur la broche 10) : l'octet vient de `read_data` au début de chaque octet, chaque front descendant décale le bit suivant ; `read_data` qui rend `None` relâche la ligne, que le tirage du pad fixe. La lecture finit à CS haut ou D/C bas. Une ligne relâchée (`None`) : RST lit haut (tirage interne du contrôleur), CS désélectionne, SCLK, MOSI et D/C n'horlogent rien. | La sonde de FreeInk (RW §1 point 3) écrit `0x71`, `0x70` et lit la ligne demi-duplex ; le SSD1677 n'y répond pas et la ligne flotte au tirage : verdict SSD1677, comme sur l'appareil. |
| 2d-D5 | **RST** (`GpioOutput`, broche 5) : bas → `set_reset(now, true)` ; haut ou relâché → `false`. **BUSY** (`GpioInput`, broche 6) : toujours piloté, `busy_pin(now)` ; `next_change` rend l'instant où BUSY retombe. **Temps propre** (`Clocked`) : `next_deadline` et `advance` de la puce. | FreeInk attend BUSY par interruption sur front (SP2a §2) : le front doit tomber à son instant, pas à la lecture suivante. |
| 2d-D6 | **Échelles, batterie, bouton d'allumage.** `ButtonLadder` et `Battery` implémentent `AnalogSource` (leur tension ne dépend que de leur état). Un nouveau `chips::PushButton { actif: Level }` implémente `GpioInput` : enfoncé, il pilote son niveau actif ; relâché, il laisse la ligne au tirage du pad (`INPUT_PULLUP` de FreeInk, RW §1 point 5). | Ce que voient GPIO0–3 (RW §2). |
| 2d-D7 | **Les tensions des échelles sont celles qui rendent les codes mesurés** sous l'ADC de SP2b : pour chaque moyenne de FreeInk (Back 3512, Confirm 2694, Left 1493, Right 5, Up 2242, Down 5), la tension que la courbe d'ESP-IDF donne à ce code à 12 dB sur l'eFuse du X4 ; repos 3 300 mV. Un test reconvertit chaque niveau par l'ADC de la machine et exige le bouton que décodent les seuils de FreeInk. | La droite 0–2 500 mV de SP2a §7 ne revient pas au code d'origine sous l'ADC calibré de SP2b (RW §3). |
| 2d-D8 | **Les entrées de la carte.** `emu-core::signals` gagne `BoardInput { target: u16, value: u32 }` et le trait `BoardInputs` (`check`, `apply(input, at)`). La machine reçoit `attach_board_inputs` et `send_board_input(at, input)` : même contrôle de date que `send_console`, refus d'une cible que la carte ne connaît pas, entrée journalisée (`AcceptedInput::Board`), appliquée **par la machine à sa date**, y compris pendant un sommeil profond, puis broches réévaluées. L'interface (appui à l'instant présent) et le rejeu (toutes les entrées remises à la mise sous tension) passent par ce seul chemin. Bornes : 16 384 entrées de carte journalisées par vie, 16 384 en attente. | ADR 0007 : la machine tient le journal de ce qu'elle accepte ; un rejeu remet ses entrées d'avance. Un appui appliqué par le frontal hors de la machine aurait un chemin différent au rejeu : la réévaluation des broches à l'instant de l'appui (qui fixe le niveau d'où se compte un front) diffèrerait. |
| 2d-D9 | **Cibles de la carte du X4** : les boutons du profil dans leur ordre (valeur 1 enfoncé, 0 relâché), puis `usb-power` (1 câble branché, 0 absent). Le texte du profil est dans la session : la numérotation y est stable. | Une cible est un numéro que seule la carte interprète ; le SoC n'apprend rien des boutons. |
| 2d-D10 | **Format de session 2** : `inkemu-session 2`, une entrée d'étiquette 4 (`u64` instant en cycles, `u16` cible, `u32` valeur) ; borne de 16 384 entrées de carte ; le lecteur lit les formats 1 et 2, l'enregistreur écrit le 2. Le rejeu reconstruit la carte câblée depuis le profil de la session et fait vérifier chaque entrée de carte par elle avant de la remettre. | Le report de SP1b-4b et ADR 0007 : les boutons rejoignent le journal et changent le numéro du format. Un fichier 1 ne contient aucune entrée de carte : le lire coûte une ligne. |
| 2d-D11 | **Réveil GPIO du sommeil profond** : `RTC_CNTL.GPIO_WAKEUP`, `EXT_WAKEUP_CONF` (filtre), `GPIO_PINn_WAKEUP_ENABLE` et `GPIO_PINn_INT_TYPE` (4 bas, 5 haut) pour GPIO0–5, `WAKEUP_ENA` bit 2. À l'entrée en sommeil, la machine évalue les broches de réveil (niveau d'une puce, sinon le tirage du pad en sommeil) et leur prochain changement ; elle se réveille au premier instant où l'une est à son niveau (aussitôt si elle y est déjà), ou à l'échéance du timer s'il vient avant ; elle latche la cause GPIO et `GPIO_WAKEUP_STATUS`, puis fait le reset de cause 5 de SP1b-3f. Un sommeil profond dont aucun réveil ne peut venir de la machine seule (ni timer, ni entrée de carte en attente, ni changement annoncé par une puce) **finit l'exécution** comme aujourd'hui (`RunEnd::DeepSleep`), et l'interface reprend la vie à la première entrée acceptée. | Le firmware d'origine s'arrête là (SB) ; CrossPoint s'y endort sur câble (RW §1 point 11). |
| 2d-D12 | **Maintien des broches** : `RTC_CNTL.PAD_HOLD` (GPIO0–5), `DIG_PAD_HOLD` et le maintien automatique en sommeil (`DG_PAD_AUTOHOLD_EN` ; **non modélisé à la livraison** : `DIG_ISO` est accepté sans effet, seul le bit de maintien propre à une broche garde sa sortie, ADR 0013 point 5) ; une broche maintenue garde sa sortie (niveau, validation) à travers un reset du cœur et ignore les écritures jusqu'à sa libération. L'isolement des broches en sommeil (`esp_sleep_config_gpio_isolate`) n'est pas modélisé au-delà des registres de l'IO MUX : une broche garde en sommeil la configuration de sommeil (`SLP_SEL`, `MCU_*`) ou, sans elle, sa configuration (hypothèse, à vérifier contre ESP-IDF 5.5.2, RW §4). | FreeInk maintient RST haut et GPIO13 bas à travers le sommeil (RW §1 points 4 et 11) ; « a held pad silently ignores the drive ». |
| 2d-D13 | **Alimentation de la carte.** `emu-core::signals` gagne le trait `Supply` (`powered(at) -> bool`). La machine en reçoit un, le consulte après chaque changement d'une sortie de broche, chaque entrée de carte appliquée et à la mise sous tension ; quand il rend `false`, l'exécution s'arrête à cet instant (`StopReason::PowerOff`, `RunEnd::PowerOff`) et la machine ne repart plus. Le X4 est alimenté tant que le câble est branché, que le verrou GPIO13 n'est pas piloté bas (relâché, il s'auto-maintient) ou que le bouton d'allumage est enfoncé. | RW §1 points 9 et 11 : sur batterie, CrossPoint coupe le verrou avant de dormir et l'appareil s'éteint au relâchement du bouton ; l'appui suivant est un démarrage à froid. |
| 2d-D14 | **Le geste d'allumage.** Le profil désigne le bouton qui allume l'appareil et la durée de l'appui (`[power] key = "power"`, `hold_ms = 1000`, hypothèse ; **remplacée par 1 500 ms à la mesure** : sur le modèle, le firmware d'origine 3.0.2 s'éteint quand le bouton est lâché à 1 000 ms et reste allumé à 1 200 ms, le 2026-10-15 ; voir ADR 0013 point 4). Toute mise sous tension d'un appareil qui en a un remet deux entrées de carte datées : appui à 0, relâchement à `hold_ms` ; et `usb-power` à 0 si l'exécution l'a demandé (`--usb-power`, case du dialogue de démarrage). `inkemu run --power-hold <ms>` change la durée, `0` n'appuie pas. Dans l'interface, une vie éteinte (`PowerOff`) se rallume par le bouton d'allumage de l'appareil ou par Démarrer, avec la flash courante. | Sur batterie, l'appareil ne démarre qu'avec le bouton tenu, et CrossPoint vérifie qu'il l'est encore (RW §1 point 10). Le geste est daté : il entre dans la session comme tout appui. |
| 2d-D15 | **Le câblage est dans le profil** (`[wiring]`) : bus de l'écran (SCLK, MOSI, CS, D/C, RST, BUSY, contrôleur et sa configuration nommée), échelles (broche, repos, boutons avec leur tension), bouton d'allumage (broche, niveau actif), batterie (broche, résistances, tension de la cellule), verrou d'alimentation, détection USB ; chaque bouton y porte son placement (bord, ordre) ; et la rotation du panneau. Le chargement refuse, en nommant la clé : une broche absente du SoC, une broche prise deux fois, une entrée analogique hors des canaux de l'ADC1, deux boutons du même nom ou à la même place, une échelle que `ButtonLadder::new` refuse. Un profil sans `[wiring]` (le X3) donne une machine nue et la mire. | Spec globale §3.3 : ajouter un appareil revient à écrire un profil. `host::catalog` lit le placement et la rotation au profil au lieu de ses constantes. |
| 2d-D16 | **Le panneau du X4 pour l'interface** : `describe` donne 800 × 480, la rotation du profil (90°, RW §1 point 7), `origin: Device` et `approximate` = `Ssd1677Config::is_approximation()` ; `poll` rend une image quand la génération de `PanelImage` a augmenté : ses pixels tels quels, `Full` ou `Partial` selon le genre de rafraîchissement, sans région, la durée tirée du début et de la fin de l'affichage (`PanelImage` gagne `started`). | SP2c §4 ; SP2a A9/M4 : l'interface doit dire « approximation ». |
| 2d-D17 | **Teintes continues.** L'optique de SP2a donne n'importe quelle valeur de 0 à 255 (par exemple 153 et 128 pour la LUT de gris d'usine, RW de SP2a §6) ; les arrondir à quatre niveaux confondrait deux gris. `PanelDescription.gray_levels` devient `shades: Shades { Levels(u8), Continuous }` ; l'encodage IPC met 0 à l'octet 6 pour « continu » et passe à la version 2 (`FRAME_VERSION`). | Le contrat de SP2c imposait des niveaux régulièrement espacés, que le modèle ne produit pas. |
| 2d-D18 | **La mention « approximation ».** `PanelDescription.approximate: bool` ; tant qu'il est vrai, l'interface affiche sous l'écran « Rendu approché : paramètres optiques non mesurés » (EN : « Approximate rendering: optical parameters not measured »). Testé côté `host` (le X4 le déclare, la mire non) et côté interface (affiché si et seulement si vrai, dans les deux langues). | SP2a A9/M4. |
| 2d-D19 | **Les boutons du X4 atteignent le firmware et sont journalisés** : `set` envoie l'entrée de carte à l'instant présent de la machine. Le refus `NotInSession` (2c-C9) disparaît pour eux : un enregistrement les tient. Un rejeu refuse toujours toute entrée. | 2c-C9 attendait le format 2. |
| 2d-D20 | **Les scripts de console appuient** : `press <bouton>` et `release <bouton>`, vérifiés contre les boutons du profil au chargement du script. | La CLI doit pouvoir tout ce que fait l'interface (préférence « interface d'abord, CLI mince ») et la CI teste par la CLI. |
| 2d-D21 | **Firmware de référence `x4_panel_buttons`** (à nous, Apache-2.0, construit dans `espressif/idf:v5.5.2` comme les autres) : pilote ESP-IDF de GP-SPI2 sur les broches du X4, CS et D/C par GPIO, BUSY par interruption GPIO, séquence d'initialisation écrite d'après la fiche technique ; dessine une image connue en rafraîchissement complet, puis lit les échelles (`adc_oneshot`, 12 dB) et GPIO3 et écrit le nom de chaque bouton enfoncé ; Confirm dessine une seconde image en rafraîchissement rapide ; Power s'endort avec réveil GPIO3 bas et, réveillé, écrit sa cause et dessine une troisième image. Un test de `boards` (job `reference-boot`) compare chaque image du panneau pixel par pixel à l'attendu, appuie par entrées de carte datées, puis enregistre et rejoue la même exécution (format 2, même empreinte de trace). Un test de `run_cli` fait de même par un script de console. | Une preuve en CI de tout le chemin, sur un firmware dont nous avons le droit de figer la sortie. |
| 2d-D22 | **CrossPoint 1.6.5 et le firmware d'origine, en local seulement** (`real_firmware.rs`, hors CI) : CrossPoint allumé par le geste, sans carte, publie l'écran « SD card error » ; le test vérifie qu'une image est publiée, que son texte tourné à 90° est dans la moitié haute du portrait, et écrit un PNG sous `target/` pour l'œil. Le firmware d'origine allumé par le geste : le test consigne ce qu'il fait et le fige. | Leurs binaires sont ceux de l'utilisateur ; leur sortie n'est pas à nous. |
| 2d-D23 | **Les machines nues restent.** `DeviceProfile::instantiate*` rend toujours un SoC sans puce (fenêtres du lockstep contre QEMU, tests qui branchent leurs propres puces) ; la nouvelle entrée `boards::Wired::power_on` rend la machine câblée et ses poignées (panneau, boutons, alimentation). `inkemu run`, `inkemu replay`, `host` et `session` prennent la machine câblée. | QEMU n'a aucune de ces puces ; `reference_signals` (SP2b) branche les siennes. |

## 3. Architecture

```
emu-core::signals   Level, Nanos, GpioInput, GpioOutput, SpiTarget, AnalogSource, Clocked (SP2b)
                    + BoardInput, BoardInputs, Supply (SP2d)
chips               ssd1677::lines   Ssd1677Lines : SpiTarget, Clocked ; ses rôles RST/BUSY/SCLK/MOSI/DC/CS
                    analog           ButtonLadder, Battery : AnalogSource ; PushButton : GpioInput
soc-esp32c3         inputs           AcceptedInput::Board, bornes ; machine : attach_board_inputs, send_board_input
                    rtc_cntl/sleep   réveil GPIO ; io : maintien des broches ; machine : attach_supply, PowerOff
boards              profile          [wiring], [power], rotation, placement des boutons, validation
                    wired            Wired::power_on → { machine, panel, buttons, power } ; X4Board : BoardInputs, Supply
session             format 2, Replay sur Wired
host                catalog          ports du profil câblé ; ports_x4 : PanelSource, ButtonPort ; Shades, approximate
console-script      press, release
emu-cli             --power-hold, --usb-power
app                 teintes continues, mention « rendu approché », état « éteint », bandeau de mire retiré pour le X4
```

**Flux d'un appui** : l'interface appelle `ButtonPort::set("confirm", true)` → `X4Buttons` envoie
`BoardInput { target: 1, value: 1 }` à `machine.send_board_input(maintenant)` → la machine journalise, puis, à
cette date, appelle `X4Board::apply` (l'échelle A passe au niveau de Confirm) et réévalue les broches → la
conversion suivante de l'ADC rend le code de Confirm (2 694 environ), que le firmware décode.

**Flux d'une image** : le firmware écrit par GP-SPI2 → `Ssd1677Lines::transfer` → `write_command`/`write_data`
→ `0x20` lance l'affichage, BUSY monte → `Clocked::next_deadline` → la machine avance la puce à la fin → BUSY
retombe (front, interruption) et `PanelImage` change de génération → `PanelSource::poll` rend la `PanelFrame`.

## 4. Le profil du X4 (extrait)

```toml
[wiring.display]
controller = "ssd1677"
config = "gdeq0426t82"          # Ssd1677Config::gdeq0426t82()
sclk = 8
mosi = 10
cs = 21
dc = 4
rst = 5
busy = 6
rotation = 90                   # sens horaire, RW §1 point 7

[[wiring.ladder]]
pin = 1
idle_mv = 3300
buttons = [
  { id = "back",    mv = …, edge = "bottom", order = 0 },
  { id = "confirm", mv = …, edge = "bottom", order = 1 },
  { id = "left",    mv = …, edge = "bottom", order = 2 },
  { id = "right",   mv = …, edge = "bottom", order = 3 },
]

[[wiring.ladder]]
pin = 2
idle_mv = 3300
buttons = [
  { id = "up",   mv = …, edge = "right", order = 0 },
  { id = "down", mv = …, edge = "right", order = 1 },
]

[wiring.button.power]
pin = 3
active = "low"
edge = "top"
order = 0

[wiring.battery]
pin = 0
top_ohms = 10000
bottom_ohms = 10000
cell_mv = 3900                  # ASSUMPTION

[wiring.power]
latch = 13                      # bas : coupe la batterie
usb_detect = 20                 # haut : câble branché

[power]
key = "power"
hold_ms = 1500                  # measured on the model (was 1000, an assumption): ADR 0013 point 4
```

Les `…` sont calculés à la tâche des échelles (2d-D7). L'ordre des boutons dans la liste des ports, donc leurs
cibles, est : `power`, puis les échelles dans l'ordre du fichier.

## 5. Le contrat des ports, modifié

```rust
pub enum Shades { Levels(u8), Continuous }
pub struct PanelDescription { width, height, shades: Shades, rotation, origin, approximate: bool }
pub struct FrameParts { width, height, shades: Shades, generation, refresh, region, duration_nanos, pixels }
pub struct ButtonSpec { id: String, edge, order }      // lu au profil
pub trait Recipe: Send {
    fn power_on(&mut self, flash: Option<Vec<u8>>, recorded: bool) -> Result<Life, PowerOnError>;
}
pub struct Life { machine, panel, buttons }            // inchangé ; la vie éteinte finit par RunEnd::PowerOff
```

`ButtonError::NotInSession` disparaît (2d-D19). `PanelFrame::encode` : octet 6 = nombre de niveaux, 0 pour continu ;
`FRAME_VERSION` = 2.

## 6. Tests et critères d'acceptation

1. **Puces** (`chips`, unitaires, sans SoC) : la séquence de FreeInk passée par `SpiTarget` (sélections, D/C par
   transfert, sélection sans transfert) donne la même RAM et la même image que les appels de commande de SP2a ;
   la sonde bit-bang de FreeInk (`0x71`, `0x70`, cinq octets lus) lit la ligne relâchée et la puce consigne deux
   commandes inconnues ; une lecture bit-bang de `0x2F` rend l'octet d'état bit à bit ; BUSY annonce sa retombée ;
   RST relâché ne réinitialise pas ; `ButtonLadder`, `Battery`, `PushButton` répondent selon leur état.
2. **SoC** (`soc-esp32c3`, unitaires) : une entrée de carte datée est appliquée à sa date, journalisée, refusée
   au-delà des bornes ou pour une cible inconnue, appliquée pendant un sommeil profond ; réveil GPIO bas et haut,
   immédiat si le niveau y est déjà, en concurrence avec le timer, cause et statut lus comme ESP-IDF les lit ;
   broche maintenue à travers un reset du cœur ; coupure d'alimentation à l'instant d'une écriture de broche et à
   celui d'une entrée de carte.
3. **Profil** (`boards`) : le X4 se charge ; chaque refus de 2d-D15 a son message ; les tensions des échelles
   reviennent, par l'ADC de la machine, aux boutons que décodent les seuils de FreeInk.
4. **Session** : format 2 lu et écrit, borne des entrées de carte, fichier 1 encore lu, entrée de carte inconnue
   refusée au rejeu ; une session avec appuis rejouée en tranches de 1, 7, 997 et 7 919 cycles donne le même
   résultat ; appuis pendant un sommeil profond.
5. **Référence** (CI, job `reference-boot`) : `x4_panel_buttons` — trois images égales pixel par pixel à l'attendu,
   les noms des sept boutons écrits dans l'ordre des appuis, le réveil par GPIO3 ; enregistrement puis rejeu
   identiques ; empreinte de trace épinglée.
6. **Interface** : `host` (mire pour le X3, ports câblés pour le X4, `approximate`, teintes continues encodées,
   appui journalisé pendant un enregistrement, rallumage d'une vie éteinte) ; `app` (décodage de la version 2, la
   mention en FR et en EN affichée si et seulement si `approximate`, bandeau de mire absent pour `origin: device`,
   état éteint).
7. **Local** : CrossPoint 1.6.5 jusqu'à « SD card error » (2d-D22) ; firmware d'origine allumé par le geste.
8. **Non-régression** : toutes les empreintes épinglées des firmwares de référence inchangées, sauf celles que le
   câblage change avec une raison écrite ; les trois fenêtres du lockstep inchangées (machine nue).

## 7. Hors périmètre, et héritiers

| Sujet | Héritier |
|---|---|
| Carte SD en SPI, stockage hôte (image brute, dossier en FAT), son contenu dans les sessions, l'écran d'accueil de CrossPoint | **SP2e** (nouveau, avant le 2027-01-01) |
| RAM brute du contrôleur et compteur de rafraîchissements partiels (besoins de SP2a sans suite dans SP2c) | **SP4** (débogage) : ce sont des vues de débogage, pas ce que voit l'utilisateur de l'appareil |
| Brancher ou retirer le câble pendant une vie, régler la tension de la batterie depuis l'interface | SP4 ; la cible `usb-power` existe déjà, seule l'interface manque |
| X3 (UC8253, I2C) | SP3 |
| Réveil d'un sommeil léger par GPIO ; la bordure (VBD) dans l'image | refusés comme aujourd'hui / plus tard, si un firmware en dépend |

## 8. SP2e en une phrase

Une carte SD en mode SPI (`chips::sdcard`), sur la broche CS 12 et la MISO 7 du bus partagé, adossée à une image
brute puis à un dossier de l'hôte, dont le contenu de départ entre dans la session comme la flash ; c'est ce qui
mène CrossPoint à son écran d'accueil.

## 9. Ce qui dépend de SP2b (vérifié sur `main` à `0a04195`, tâche 0) et de SP2e

SP2b est fusionné (PR #18). Chaque point a été relu dans `crates/emu-core/src/signals.rs`,
`crates/soc-esp32c3/src/periph/io/mod.rs`, `machine.rs`, `periph/rtc_cntl/` et la section « Pour SP2d » de la note
de SP2b.

| Point | Constaté | Effet sur SP2d |
|---|---|---|
| `Clocked`, `Esp32C3::attach_clocked` | publics ; une puce cadencée est avancée à ses échéances pendant l'exécution, un `WFI`, un sommeil profond et à travers un reset du cœur, avant tout autre appel à une puce, une fois par instant (une échéance restée due est reprise au cycle suivant) ; 1 000 avances sans progrès la nomment (`ClockedChipStuck`) ; un sommeil profond ne finit faute de réveil qu'une fois qu'aucune puce n'a d'échéance | 2d-D5 inchangé ; le SSD1677 n'a d'échéance que pendant une opération : il ne retient pas un sommeil |
| Plusieurs attaches sur une broche | une seule `GpioInput` par broche (`InputTaken`) ; des `GpioOutput` en nombre ; les broches d'un `SpiTarget` ne sont pas réservées | les rôles du SSD1677 se branchent tels quels (MOSI : une sortie observée, une entrée pilotée) |
| Fronts de GP-SPI2 sur les `GpioOutput` | aucun : une broche routée sur `FSPICLK`/`FSPID` est entendue au niveau de repos du signal, pas bit à bit (`IoMatrix::settle`, `signal_high`) | le bit-bang ne voit que le trafic écrit par GPIO |
| `next_change` | demandé seulement quand `GPIO_PINn.INT_TYPE` ≠ 0 ; en sommeil profond, seules les puces cadencées sont avancées, les broches ne sont pas évaluées | SP2d ajoute l'évaluation des broches de réveil en sommeil et leur `next_change` (2d-D11) |
| `inputs_changed` | appelle `IoMatrix::settle` | l'application d'une entrée de carte appelle le même `settle` |
| Conversion de l'ADC | `Calibration::reading` est `pub(crate)` | les tensions des échelles se vérifient par une conversion réelle de la machine (un petit programme qui lance une conversion), sans rien exposer |
| `PAD_HOLD`, `DIG_PAD_HOLD`, `GPIO_WAKEUP`, `EXT_WAKEUP_CONF`, `DIG_ISO` | stockés sans effet (table de `rtc_cntl/mod.rs`) ; `WAKEUP_ENA` bit 2 est signalé puis ignoré (le sommeil ne se fie qu'au timer) | SP2d donne leur effet à `PAD_HOLD`, `DIG_PAD_HOLD`, `GPIO_WAKEUP` et `EXT_WAKEUP_CONF` (2d-D11, 2d-D12) et retire le refus ; `DIG_ISO` reste accepté sans effet (2d-D12, ADR 0013 point 5) |

**SP2e (carte SD, PR #19, non fusionné).** Sa spec §9 liste ses ajouts aux tâches 0, 4, 9, 11 à 15, 17 et 18 :
`chips::sdcard::lines::SdLines` (`SpiTarget` d'un `SdSlot`, base 1 GHz), `[wiring.sd] cs = 12, miso = 7`
(SCLK et MOSI partagés avec l'écran), `WiredOptions.sd: Option<SdStart>` et une poignée `SdHandle`, la carte dans le
format de session 2, `inkemu run --sd-image`, `Setup.sd`, la boîte de démarrage, et CrossPoint jusqu'à son écran
d'accueil avec une image vierge. SP2d prépare les points d'accroche sans dépendre de ses types : la validation du
profil admet le partage de SCLK et MOSI par deux cibles SPI, `WiredOptions` et `Setup` sont des structures qu'on
étend, et le format 2 garde ses entrées dans des sections étiquetées qu'une section de carte peut suivre. La carte
n'est branchée qu'après la fusion de SP2e.
