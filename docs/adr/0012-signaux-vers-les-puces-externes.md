# ADR 0012 — Signaux entre le SoC et les puces externes, et broches de l'ESP32-C3

- Date : 2026-10-15
- Statut : accepté (plan SP2b)

## Contexte

La spec globale (§3.2) veut que les puces externes ne connaissent pas le SoC : elles ne voient que des signaux (SPI,
I2C, GPIO, ADC), et seul `boards` relie le SoC aux puces. Le sous-projet 2 avance en chantiers parallèles : SP2a écrit
les puces (`chips` : SSD1677, échelle de boutons, batterie) avec une interface au niveau des commandes ; SP2b modélise
les périphériques du SoC qui portent ces signaux (matrice GPIO, IO MUX, GP-SPI2, I2C0, SAR ADC) ; SP2d branchera les
deux dans `boards`. Il fallait un contrat que ni le SoC ni les puces ne possèdent, et décider où vit l'état des
broches, que lisent quatre périphériques à la fois (GPIO, SPI2, ADC, et le contrôleur d'interruptions via la source
GPIO).

## Décision

1. **Quatre traits neutres dans `emu-core::signals`**, sans aucune notion de SoC : `GpioInput` (une puce pilote une
   ligne que le SoC lit ; elle dit aussi quand son niveau changera de lui-même), `GpioOutput` (une puce observe une
   ligne que le SoC pilote), `SpiTarget` (une cible d'un bus SPI dont le SoC est maître : sélection, puis transferts
   entiers avec le niveau de la ligne données/commande), `AnalogSource` (une tension sur une entrée analogique). Le
   temps qu'ils reçoivent est le temps d'appareil en nanosecondes (`Nanos`), pas en cycles d'une horloge du SoC.
   `Level` dit un niveau logique. Une puce qui tient plusieurs lignes se partage entre plusieurs branchements par
   `Rc<RefCell<_>>`, pour lequel `emu-core` implémente les quatre traits. Pas de `Send` : la machine ne l'est pas.
2. **Le SoC branche par broches, la carte dit lesquelles.** `Esp32C3::attach_input`, `attach_output`, `attach_spi`
   (horloge, MOSI, MISO, CS, DC) et `attach_analog` prennent des numéros de broche ; le SoC résout lui-même, avec ses
   registres, ce que chaque broche porte : un transfert de GP-SPI2 va à chaque cible dont l'horloge et le MOSI sont
   les broches où la matrice envoie FSPICLK et FSPID, et dont la broche CS est basse ; la ligne DC est le niveau de sa
   broche. Ainsi le CS logiciel (un GPIO écrit par le firmware, ce que fait CrossPoint) et le CS matériel (le signal
   FSPICS0) se traitent de la même façon. Un trafic produit en manipulant les broches une à une (bit-banging) n'est
   vu que comme des changements de niveau (`GpioOutput`).
3. **Un seul modèle pour la matrice GPIO et l'IO MUX** (`periph::io::IoMatrix`), qui répond aux deux blocs : les
   registres GPIO arrivent à leur décalage depuis `0x6000_4000`, ceux de l'IO MUX à leur décalage depuis la même
   base (`0x5000` et au-delà). Il garde les registres, les signaux de sortie des périphériques modélisés et les
   branchements, et calcule chaque broche : pilotée (niveau ou drain ouvert), sinon le niveau d'une puce, sinon le
   tirage. GP-SPI2, le SAR ADC et I2C0 le reçoivent de `Peripherals::deliver` quand leur opération se termine, comme
   tout ce qu'un périphérique doit à un autre passe par la machine (ADR 0003) ; le contexte ne grossit pas.
4. **Le temps des entrées.** Une broche branchée en entrée est relue à chaque lecture de `GPIO.IN`, après chaque
   changement de configuration ou de sortie, à l'instant que sa puce annonce (`next_change`, un appel `Event::Pins`
   que le modèle n'arme que si une interruption GPIO est configurée), et quand la carte le demande
   (`Esp32C3::inputs_changed`, après une entrée de l'utilisateur). Une puce dont le niveau ne dépend que de son état
   et du temps donne donc une exécution déterministe.
5. **Les opérations ont leur durée.** Un transaction SPI dure ses bits à l'horloge de `SPI2.CLOCK`, une séquence I2C
   ses bits à celle de `I2C0`, une conversion 25 cycles de SAR_CLK ; la cible reçoit le transfert à l'instant où il
   finit. Ce qui n'est pas modélisé est refusé par un avertissement nommé, jamais ignoré.

## Conséquences

- Les puces de SP2a n'ont qu'à implémenter ces traits (par un adaptateur si leur interface est au niveau des
  commandes) ; SP2d n'écrit que le câblage : lire le profil, appeler `attach_*`.
- Les branchements ne font pas partie d'un enregistrement de session : la carte les reconstruit du profil. Une entrée
  de l'utilisateur sur une puce (un bouton) doit entrer dans le journal des entrées pour que le rejeu la reproduise :
  c'est le travail de SP2d et de SP2c.
- Ni QEMU ni la puce ne disent tout (contention, broche flottante, durée exacte des phases CS) : ces points sont des
  `ASSUMPTION:` listées dans la note de passation de SP2b.
- **Révision** : à rouvrir pour I2C (SP3 : X3, cibles I2C), pour le bit-banging si un firmware en dépend pour
  fonctionner, et si un périphérique non modélisé (GDMA, LEDC) doit porter un signal.
