# ADR 0013 — Les entrées de la carte, son alimentation et le X4 câblé

- Date : 2026-10-15
- Statut : accepté (plan SP2d)
- Complète : ADR 0003 (temps de l'appareil), ADR 0007 (sessions), ADR 0011 (crate hôte et ports), ADR 0012 (signaux vers les puces externes, SP2b)
- Complétée par : ADR 0014 (la carte SD, son stockage par blocs et le fichier image de l'hôte, SP2e)
- Design : `docs/superpowers/specs/2026-10-15-sp2d-x4-wiring-design.md` (décisions 2d-D1 à 2d-D23)

## Contexte

Avant SP2d, les puces du X4 existaient (SP2a : le SSD1677, les échelles de boutons, la batterie) et le SoC savait
leur parler par ses broches (SP2b, ADR 0012), mais rien ne les reliait : `inkemu run --device x4` donnait un SoC nu,
l'application de bureau (SP2c) affichait une mire et refusait les boutons hors d'une session. CrossPoint 1.6.5
s'arrêtait faute d'écran et de carte ; le firmware d'origine 3.0.2 lisait ses boutons, ne trouvait pas le bouton
d'allumage tenu et se rendormait. Un appui de bouton n'avait aucune place dans le journal d'une exécution
(ADR 0007), donc aucune dans une session. Et l'appareil n'avait pas d'alimentation : un firmware qui coupe la sienne
ne s'éteignait pas.

## Décision

1. **Le câblage est une donnée du profil** (`[wiring]`, `[power]` dans `crates/boards/profiles/<id>.toml`) : le bus
   de l'écran et son contrôleur nommé, les échelles de boutons (broche, repos, tension de chaque bouton), le bouton
   d'allumage (broche, niveau actif), la batterie (broche, pont diviseur, tension de la cellule), le verrou
   d'alimentation, la détection du câble, le lecteur de carte, la place de chaque bouton autour de l'écran et la
   rotation du panneau. Le chargement refuse, en nommant la clé, une broche absente du SoC ou prise deux fois, une
   entrée analogique hors de l'ADC1, deux boutons de même nom ou de même place, une échelle que ses puces refusent.
   `boards` est le seul à relier broches et puces (`boards::Wired::power_on`) ; les machines nues restent
   (`DeviceProfile::instantiate*`), pour le lockstep contre QEMU et les tests qui branchent leurs propres puces.
   Un profil sans câblage (le X3, SP3) donne la machine nue et la mire.
2. **Les entrées de la carte** passent par la machine, comme les octets des consoles : `BoardInput { target, value }`
   (`emu-core::signals`), `Esp32C3::send_board_input(at, input)`, même contrôle de date que `send_console`, cible
   vérifiée par la carte, entrée journalisée (`AcceptedInput::Board`) et **appliquée par la machine à sa date** (au
   premier instant qui la suit), y compris pendant un sommeil profond, les broches
   réévaluées à cet instant. L'interface (appui à l'instant présent),
   les scripts de console (`press <bouton>`, `release <bouton>`) et le rejeu n'ont que ce chemin. Les cibles du X4 :
   les boutons de `[wiring.button]` par ordre de nom (`power`), puis ceux des échelles dans l'ordre du profil, puis
   `usb-power` ; valeur 1 enfoncé ou branché, 0 relâché ou débranché. Bornes : 16 384 entrées de carte journalisées
   par vie, 16 384 en attente.
3. **L'alimentation de la carte** est un trait (`Supply`) que la machine consulte après chaque changement d'une sortie
   de broche, chaque entrée de carte appliquée et à la mise sous tension. Le X4 est alimenté tant que le câble est
   branché, que le verrou (GPIO13) n'est pas piloté bas, ou que le bouton d'allumage est enfoncé. Quand la carte coupe
   l'alimentation, l'exécution s'arrête à cet instant (`StopReason::PowerOff`, `RunEnd::PowerOff`), comme s'arrête un
   appareil sur batterie qu'un firmware éteint : `inkemu run` le dit (« the device powered off: its board cut its
   supply ») et sort en 0. Dans l'interface, l'appareil s'affiche éteint ; son bouton d'allumage lance une nouvelle vie
   avec la flash courante (un démarrage à froid, comme sur l'appareil).
4. **Le geste d'allumage** : le profil désigne le bouton qui allume l'appareil et la durée de l'appui
   (`[power] key = "power"`, `hold_ms = 1500`). Toute mise sous tension remet deux entrées datées, l'appui à 0 et le
   relâchement à `hold_ms`, et le câble à 0 quand l'exécution le demande (`--usb-power`, case du dialogue).
   `--power-hold <ms>` change la durée, `0` n'appuie pas. La durée est **mesurée**, non supposée : le design prévoyait
   1 000 ms ; sur le modèle, le firmware d'origine 3.0.2 ne tient son verrou qu'après environ une seconde de temps
   d'appareil (relâché à 1 000 ms, l'appareil s'éteint ; à 1 200 ms, il reste allumé, mesuré le 2026-10-15) :
   1 500 ms garde une marge. CrossPoint, qui vérifie l'appui 10 ms, démarre sous les deux.
5. **Le sommeil profond se réveille par une broche** (`RTC_CNTL.GPIO_WAKEUP`, `GPIO_PINn_WAKEUP_ENABLE`, niveaux bas
   et haut, GPIO0 à 5) au premier instant où une broche de réveil est à son niveau, ou à l'échéance du timer RTC si
   elle vient avant ; la cause et `GPIO_WAKEUP_STATUS` sont latchées, puis vient le reset de cause 5 (SP1b-3f). Un
   sommeil dont rien ne peut réveiller la machine seule (ni timer, ni entrée de carte en attente, ni changement annoncé
   par une puce) finit toujours l'exécution. Les broches maintenues (`PAD_HOLD`, `DIG_PAD_HOLD`, maintien automatique
   en sommeil) gardent leur sortie à travers un reset du cœur et ignorent les écritures jusqu'à leur libération.
6. **Les sessions passent au format 2** (`inkemu-session 2`) : une entrée de carte y a l'étiquette 4 (instant en
   cycles, cible `u16`, valeur `u32`), et la carte SD de départ y entre (ADR 0014). Le lecteur lit les formats 1 et 2 ;
   l'enregistreur écrit le 2. Le rejeu reconstruit la machine câblée depuis le profil de la session, **sans geste**
   (les deux entrées du geste sont dans le journal), fait vérifier chaque entrée par la carte et la remet d'avance.
7. **Le panneau du X4 pour l'interface** (`host::ports_wired`) : 800 × 480, la rotation du profil, des teintes
   **continues** (`Shades::Continuous` ; l'encodage d'une image passe à la version 2, octet 6 à 0), l'image de la
   puce telle quelle à chaque génération, `Full` ou `Partial`. Tant que ses paramètres optiques sont ceux du modèle
   (`Ssd1677Config::is_approximation`), l'interface affiche « Rendu approché : paramètres optiques non mesurés » (EN :
   « Approximate rendering: optical parameters not measured »).
8. **La preuve en CI** est un firmware de référence à nous, `tools/reference-firmware/x4_panel_buttons` (Apache-2.0,
   construit dans l'image figée comme les autres) : il tient son verrou, lit le câble et la batterie, rafraîchit le
   panneau en entier, montre chaque appui par un carré et un rafraîchissement différentiel, puis, sur batterie, lâche
   son verrou sur un appui du bouton d'allumage (l'appareil s'éteint au relâchement) et, câble branché, s'endort
   réveillé par ce bouton. `crates/boards/tests/reference_panel.rs` lit les pixels du verre et la fin de chaque
   exécution ; `run_cli` fait de même par un script de console, enregistre la session et la rejoue à l'identique.
   CrossPoint 1.6.5 et le firmware d'origine, binaires de l'utilisateur, ne tournent qu'en local
   (`crates/host/tests/real_firmware.rs`, `crates/emu-cli/tests/real_firmware.rs`).

## Conséquences

### Positives

- CrossPoint 1.6.5 démarre, monte une carte vierge et dessine son écran d'accueil ; sans carte, son écran « SD card
  error » ; le firmware d'origine reste allumé et dessine son menu. L'application de bureau les montre et leurs boutons
  atteignent le firmware.
- Un appui de bouton a la même place qu'un octet de console : journalisé, enregistré, rejoué à l'identique (même
  empreinte de trace), y compris pendant un sommeil profond.
- Ajouter un appareil câblé revient à écrire son profil ; les erreurs de câblage sont dites au chargement, clé nommée.
- Les empreintes de trace figées avant SP2d n'ont pas bougé : la machine câblée ne change rien à un firmware qui ne
  touche pas aux broches câblées.

### Négatives

- Le format des sessions change : un fichier 2 ne se lit pas avec une version antérieure (le contraire reste vrai).
- La durée du geste est une mesure sur le modèle, pas sur un appareil ; elle reste une hypothèse du profil, que
  l'utilisateur peut changer (`--power-hold`).
- Le panneau est une approximation (forme d'onde construite, optique supposée) : le rendu le dit, mais un firmware
  qui dépend d'un gris précis peut l'afficher autrement que l'appareil.
- La RAM brute du contrôleur, le compteur de rafraîchissements partiels, la batterie variable et le câble pendant une
  exécution ne sont pas encore montrés dans l'interface (héritiers SP4).

## Alternatives considérées

1. **Appliquer les appuis hors de la machine, dans le frontal** : rejeté. Le rejeu remet les entrées à la mise sous
   tension ; un appui appliqué ailleurs aurait réévalué les broches à un autre instant, et le front d'où le firmware
   compte un appui aurait changé.
2. **Un bouton d'allumage toujours « tenu »** au lieu d'un geste daté : rejeté. Le firmware d'origine et CrossPoint
   lisent le relâchement ; tenu pour toujours, aucun appui suivant ne serait possible, et rien ne serait journalisé.
3. **Arrondir les gris du panneau à quatre niveaux**, le contrat de SP2c : rejeté. L'optique du modèle produit
   d'autres valeurs (par exemple 153 et 128 pour la LUT de gris d'usine) ; arrondies, deux teintes se confondraient.
4. **Une durée de geste d'une seconde**, celle du design : rejetée après mesure, elle éteint le firmware d'origine.
