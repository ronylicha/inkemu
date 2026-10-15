# ADR 0010 — La crate hôte, l'émulateur sur un fil et les ports de l'écran et des boutons

- Date : 2026-10-15
- Statut : accepté (plan SP2c)
- Complète : ADR 0004 (fichier de flash), ADR 0006 (entrées des consoles), ADR 0007 (sessions)

## Contexte

L'utilisateur veut une version de l'émulateur qui se pilote en mode graphique avant le 2027-01-01, et a posé une
règle : « Interface d'abord » — tout se pilote depuis l'interface, la logique vit dans des crates bibliothèques, la
CLI reste mince. À la fin de SP1b-4e, ce que la CLI fait autour d'une machine (fichier de flash, lecture bornée des
entrées, composition d'une flash, ROM, sessions, boucle d'exécution) vit dans `emu-cli`, et une partie dans son
binaire (`main.rs`). Les notes de passation de SP1b-3c à SP1b-4b demandaient de le déplacer « dans une crate
hôte ».

L'interface a d'autres besoins que la CLI : une machine qui tourne pendant que l'utilisateur clique, des commandes
(pause, pas à pas, reset), une vitesse, des flux, et l'image d'un écran que SP2a modélise (SSD1677) et que SP2d
raccordera. `Esp32C3` n'est pas `Send` (`FlashDevice` n'exige pas `Send`) et `crates/soc-esp32c3` est hors du
périmètre de SP2c.

## Décision

1. **Une crate `host`** reçoit les modules de politique d'`emu-cli`, déplacés tels quels, et ce que `main.rs`
   décidait d'une exécution (`launch`). `emu-cli` les réexporte sous leurs chemins : la CLI les consomme, ses sorties
   et ses codes de sortie ne changent pas.
2. **Un moteur sans E/S** (`engine`) fait vivre une machine ; **un fil** (`device`) le porte, avec l'horloge
   murale, les fichiers et les commandes. La machine naît sur le fil, construite par une recette `Send`
   (`ports::Recipe`), et n'en sort jamais. Les commandes sont servies entre deux passes et chacune rend son résultat ;
   les mises à jour vont, dans l'ordre, à un observateur.
3. **Une vie = une mise sous tension.** Le reset est celui de `CHIP_EN` : une nouvelle machine avec la flash où la
   vie d'avant l'a laissée. Chaque fin de vie écrit le fichier de flash et la session, ou donne le verdict d'un rejeu.
4. **Les ports** (`ports::PanelSource`, `ports::ButtonPort`) sont créés avec la machine, sur le fil, et reçoivent
   la machine à chaque appel. Le contrat est celui du §4 de la spec SP2c ; `catalog::ports_for` est le seul endroit
   qui choisit les ports d'un appareil. Jusqu'à SP2d, une mire de test les implémente, et elle le dit.
5. **L'interface Tauri** est un espace de travail Cargo à part, qui appelle `host` par des commandes et reçoit les
   mises à jour par un canal ; l'image passe en octets bruts, à la demande.

## Conséquences

### Positives

- La CLI et l'interface prennent les mêmes décisions dans le même ordre, avec les mêmes refus.
- Le moteur, sans fichier, horloge ni fil, est ce que la version web (SP6) mettra dans un Web Worker.
- SP2d branche le SSD1677 et l'échelle de boutons dans `catalog` sans toucher l'interface.

### Négatives

- Les textes déplacés nomment les options de la CLI (`--flash-file`) : l'interface les montre en détail, sous un
  titre traduit choisi par la variante.
- Un appui sur un bouton qui atteint le firmware ne peut pas s'enregistrer avant le format de session 2 (SP2d) :
  le moteur le refuse pendant un enregistrement.
- Arrêter un enregistrement, c'est finir la vie : une session couvre une mise sous tension.

## Alternatives considérées

1. **Laisser la politique dans `emu-cli` et la recopier dans l'interface** : deux politiques divergeraient ;
   contraire à « Interface d'abord ».
2. **Rendre `Esp32C3` `Send`** : demande une modification de `soc-esp32c3`, hors périmètre, et contraindrait toutes
   les puces futures (une puce peut partager un `Rc` avec le SoC).
3. **Un moteur générique sur un trait de machine** : `session`, `record` et `run` travaillent sur `Esp32C3` ; la
   généricité ne servirait qu'avec un second SoC (SP7), qui la justifiera.
4. **L'enveloppe Tauri dans l'espace de travail racine** : `cargo test --workspace`, clippy et la couverture
   compileraient WebKitGTK sur des exécuteurs qui n'en ont pas les bibliothèques.
