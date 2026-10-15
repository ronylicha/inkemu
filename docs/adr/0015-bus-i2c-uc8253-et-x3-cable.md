# ADR 0015 — Le bus I2C vers les puces, l'UC8253 et le X3 câblé

- Date : 2026-10-15
- Statut : accepté (plan SP3)
- Complète : ADR 0003 (temps de l'appareil), ADR 0007 (sessions), ADR 0011 (crate `host` et ports), ADR 0012 (signaux
  vers les puces externes), ADR 0013 (entrées de la carte et alimentation), ADR 0014 (carte SD)

## Contexte

Le X3 partage le SoC, la flash et le bus de l'écran du X4, mais pas son contrôleur d'écran (un UC8253, ou un UC8279d
sur les appareils plus récents, au lieu du SSD1677), et il porte un bus I2C avec trois puces : la jauge BQ27220,
l'horloge DS3231 et le capteur de mouvement QMI8658. CrossPoint 1.6.5, une seule image pour les deux appareils,
reconnaît le X3 en sondant ces trois adresses au démarrage, puis lit la batterie à la jauge et l'heure à l'horloge
(`docs/research/sp3-x3-firmware.md` §2, §4 à §8). Avant SP3, I2C0 était modélisé sans aucune puce derrière lui : une
adresse recevait toujours un NACK. Les signaux vers les puces (ADR 0012) couvraient le GPIO, le SPI et l'analogique,
pas l'I2C. Le catalogue de `host` dessinait une mire à la place de l'écran du X3.

## Décision

1. **Un trait I2C au niveau des conditions** : `emu_core::signals::I2cTarget` (`address`, `write`, `read`, `stop`), le
   temps de l'appareil passé à chaque appel. Pas de modèle au niveau du bit : tout ce que font ESP-IDF et `Wire`
   (CrossPoint) s'exprime par conditions, et un pas par front d'horloge ne donnerait aucun comportement de plus.
2. **I2C0 atteint les puces par la matrice GPIO**, comme GP-SPI2 (ADR 0012) : une puce est jointe à une paire de pads ;
   elle n'est atteinte que si les sorties SCL et SDA d'I2C0 sont routées vers ces pads, et le maître entend l'entrée
   SDA que la matrice lui donne, le ET câblé des puces qui tirent la ligne. Une ligne que personne ne tire se lit au
   niveau de son pad : haut grâce aux résistances de la carte que le profil déclare (`[wiring.i2c] pull_up`, aussi
   `[wiring.display] mosi_pull_up` pour la ligne de données de l'écran du X3), quels que soient les tirages internes
   que le firmware choisit. Un firmware qui route ailleurs ne parle à personne et lit un NACK.
3. **Une condition à la fois, à l'instant où elle se termine** : avec des puces jointes, I2C0 s'arrête à chaque START,
   octet et STOP et appelle la puce à cet instant ; les effets visibles du firmware (FIFO, interruptions, état) restent
   publiés à la fin de la commande. Une puce dont la réponse dépend du temps (l'horloge, le compteur d'échantillons)
   est ainsi interrogée quand l'octet passe. **Sans puce, I2C0 garde son chemin d'avant SP3** : les empreintes de trace
   du X4 et des firmwares de référence ne bougent pas.
4. **L'UC8253 est un modèle au niveau des commandes** dans `chips`, sur le modèle du SSD1677 : la disposition de
   l'UC8179 que FreeInk suit, deux variantes (UC8253, UC8279d), deux plans de RAM, la fenêtre partielle, `DDX`,
   `N2OCP`, le mode KW avec ses LUT en registres, la fréquence de `PLL`, `BUSY_N` actif bas. Ses durées sont des
   paramètres ; celles que la fiche ne donne pas sont des hypothèses écrites.
5. **Un seul verre** : l'image, son type de rafraîchissement, sa provenance et le modèle optique sortent de
   `chips::ssd1677` vers `chips::glass`, partagés par les deux contrôleurs, et l'adaptateur de lignes devient générique.
   Les chemins publics du SSD1677 restent (réexports). L'interface montre l'image de l'un ou de l'autre par la même
   poignée ; elle est marquée approximation pour les deux.
6. **L'heure du DS3231 est celle de l'appareil** : elle part d'un instant du profil et avance avec le temps de
   l'appareil, jamais avec l'horloge de l'hôte ; une écriture de l'heure fixe une nouvelle origine. Une session rejoue
   donc la même heure.
7. **Le X3 est câblé par son profil** (`[wiring]` de `x3.toml`), comme le X4 (ADR 0013) : contrôleur et configuration,
   échelles, bouton d'allumage, lecteur de carte avec son alimentation commutée par GPIO13, bus I2C et une table par
   puce. Il n'a ni verrou d'alimentation ni détection du câble : `--usb-power` est refusé pour lui, en le disant ; il
   s'éteint en sommeil profond, réveillé par son bouton.
8. **Le format des sessions ne change pas** : les puces partent des paramètres du profil, qui ne changent pas pendant
   une vie ; le rejeu reconstruit la machine câblée et reprend la poignée I2C.

## Conséquences

### Positives

- CrossPoint 1.6.5 reconnaît le X3 et, au second démarrage, atteint son écran d'accueil (test local).
- Le firmware de référence `x3_panel_sensors` prouve en CI, au pixel près et valeur par valeur, le chemin complet :
  pilote `i2c_master` d'ESP-IDF, matrice, I2C0, trois puces, GP-SPI2, UC8253, sommeil profond et réveil.
- Une autre puce I2C (un autre appareil) se branche sans toucher au SoC : un `I2cTarget` et une entrée de profil.

### Négatives

- I2C0 a deux chemins (avec et sans puce) ; ils partagent le calcul des durées, mais un changement de l'un doit être
  porté à l'autre. Un test croisé (`the_same_list_leaves_the_same_on_a_bus_with_and_without_chips`) passe les mêmes
  listes sur les deux et exige les mêmes interruptions, au même instant, avec les mêmes bits `DONE`, état du bus et
  octets ; il a fait aligner le `READ` de zéro octet et la liste qui ne dure rien. Le chemin sans puce disparaîtra
  quand une raison écrite permettra de déplacer les empreintes.
- La géométrie réelle du verre du X3 et ses durées sont supposées jusqu'à une mesure sur un appareil (SP8).
- Donner l'heure de l'hôte à l'appareil, ou une batterie qui se vide, demandera un format de session 3 (SP4).

## Alternatives considérées

1. **Un modèle I2C au niveau du bit**, les puces voyant SCL et SDA comme des lignes : rejeté, un coût par front sans
   comportement de plus, et aucune puce du X3 ne dépend d'un front isolé.
2. **Appeler les puces à la fin de chaque commande d'I2C0** : rejeté, une commande dure des millisecondes et l'heure
   lue serait celle de sa fin, pas celle de l'octet.
3. **Un seul chemin d'I2C0, avec ou sans puce** : rejeté pour SP3, il déplaçait les empreintes de trace de tous les
   firmwares de référence qui touchent I2C0 sans raison de comportement.
4. **L'heure de l'hôte pour le DS3231** : rejeté, une exécution ne serait plus reproductible et une session ne se
   rejouerait plus seule ; elle reviendra comme un choix de l'utilisateur enregistré dans la session (SP4).
5. **Garder la mire du X3 et câbler plus tard** : rejeté, c'est le but de SP3.
