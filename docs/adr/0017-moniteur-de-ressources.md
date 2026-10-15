# ADR 0017 — Le moniteur de ressources : des comptes toujours tenus, lus sans effet, et une consommation dite de la fiche technique

- Date : 2026-10-15
- Statut : accepté (plan SP4b)
- Complète : ADR 0003 (temps de l'appareil), ADR 0007 (sessions), ADR 0011 (émulateur sur un fil), ADR 0016 (débogueur)
- Design : `docs/superpowers/specs/2026-10-15-sp4b-resource-monitor-design.md` (décisions 4b-D1 à 4b-D13)

## Contexte

La spec globale (§6.1) demande un moniteur de ressources qui « ne montre que des valeurs mesurées » et dit quand une
valeur n'est pas mesurable : la mémoire, la charge du processeur hors `wfi`, la fréquence, le rapport de vitesse, la
batterie et la consommation « datasheet signalée comme telle ». Trois questions se posent :

- **Quand compter.** Un compte ouvert à l'ouverture du panneau ne dirait pas le démarrage. Un compte permanent coûte à
  chaque exécution, même quand personne ne regarde.
- **Comment observer sans changer.** La machine est déterministe (§3.1) et une session se rejoue à l'identique
  (ADR 0007) : observer ne doit rien changer.
- **Que dire de la consommation.** La fiche technique de l'ESP32-C3 ne donne de valeur qu'à 160 et 80 MHz, et aucune
  par domaine d'alimentation ; or six registres d'alimentation attendaient « un modèle » (SP4).

## Décision

1. **Compter là où le temps et les événements passent déjà, jamais par instruction**, et toujours. Le temps d'attente
   en `wfi` et de sommeil profond est compté aux quatre appels de `Ctx::sleep_until` ; le temps par fréquence à chaque
   changement d'horloge (`retune`) ; une interruption prise au tour qui la prend ; une source levée quand le
   contrôleur échantillonne les niveaux (`select_interrupt`) ; un accès de registre dans `mmio_read` et `mmio_write`,
   par bloc de 2 Kio de la fenêtre des périphériques. Les pages écrites d'une RAM sont les versions de pages que la
   RAM tient déjà pour le cache de décodage (`Ram::page_versions`) : elles ne coûtent rien de nouveau. Le temps
   d'exécution est la différence.
2. **Le relevé est en lecture seule** (`Esp32C3::usage(&self) -> Usage`) et rien de ce que fait l'exécution ne lit un
   compteur. Preuve :
   - dans `soc-esp32c3`, une exécution coupée en appels, avec un relevé après chacun, a la même empreinte de trace,
     les mêmes cycles, instructions, temps d'appareil, registres et mémoire qu'une exécution d'un seul tenant, **et
     le même relevé final**, sous les deux tarifs ;
   - dans `host`, une session rejouée avec le moniteur lu toutes les deux millisecondes donne « identique ».

   Aucune empreinte figée ne bouge.
3. **Le sens est hors de la machine.** La crate `monitor`, sans E/S et compilable en WebAssembly, fait du relevé une
   vue : part du temps (exécution, attente, sommeil profond), charge en pour mille, fenêtre récente entre deux
   relevés de la même vie, pages de chaque RAM « jamais écrite », « avant le dernier reset », « depuis », sources et
   blocs nommés par le SVD, estimation de consommation. `host` garde le relevé précédent et l'usage de l'écran ;
   l'interface ne fait que formater des nombres.
4. **La consommation est celle de la fiche technique, et le dit.** ESP32-C3 Series Datasheet v2.4, tableaux 5-8
   (Modem-sleep, 160 et 80 MHz, processeur en exécution ou en attente, de « toutes horloges de périphériques
   coupées » à « toutes en marche ») et 5-9 (sommeil profond 5 µA). Aucune valeur n'est extrapolée : le temps aux
   autres fréquences (20 et 40 MHz, celles du démarrage) est dit « sans valeur de la fiche technique » et n'entre
   pas dans la charge. Puce seule, typique, sans l'écran, la carte, ni le surcoût des accès à la flash.
5. **Les six registres d'alimentation passent à SP8.** `RTC_CNTL.OPTIONS0`, `PWC`, `DIG_PWC` (hors `DG_WRAP_PD_EN`),
   `BIAS_CONF`, `SDIO_CONF` et `SYSTEM.MEM_PD_MASK` restent acceptés sans effet. Aucune source ne chiffre l'effet d'un
   domaine : seul le banc de calibration sur matériel pourra le leur donner.
6. **Ce que le moniteur ne montre pas :**
   - aucun octet de mémoire ni de flash, seulement quelles pages ont été écrites ;
   - ni tâches FreeRTOS ni tas, qui demandent l'ELF (SP4d) ;
   - ni batterie ni câble, fixés à la mise sous tension jusqu'à SP4c, qui en fait des entrées vivantes.

   Rien n'entre dans une session ni dans une option de la CLI.

## Conséquences

### Positives

- Le panneau dit ce qui s'est passé depuis la mise sous tension, démarrage compris, quel que soit le moment où il est
  ouvert.
- Fermé, le panneau ne demande rien à l'hôte ; les comptes ne touchent pas le chemin d'une instruction (coût mesuré
  par `crates/soc-esp32c3/tests/throughput.rs`, chiffres dans la note de passation de SP4b).
- `monitor` servira la version web (SP6) et les rapports des scénarios (SP4e).

### Négatives

- La mémoire est vue au grain de la page de 4 Kio : une page touchée d'un seul octet compte entière.
- Une source levée et baissée pendant une seule instruction n'est pas comptée : le contrôleur ne la voit pas non plus
  (ADR 0003).
- Un accès de registre coûte un incrément de plus, que le panneau soit ouvert ou non.
- L'estimation de consommation est une fourchette large, et partielle tant que le firmware tourne à 20 ou 40 MHz.

## Alternatives considérées

1. **Compter seulement panneau ouvert** : le coût fermé serait nul, mais le moniteur ne dirait ni le démarrage ni ce
   qui précède son ouverture, et deux ouvertures donneraient deux histoires.
2. **Compter le temps d'exécution à chaque instruction** (par fréquence) : exact sans différence, mais un test et une
   addition par instruction ; le temps passe déjà aux sommeils et aux changements d'horloge.
3. **Marquer chaque mot écrit** (empreinte au grain de 4 octets) : un test et un bit par rangement, sur le chemin le
   plus chaud ; les versions de pages existent déjà.
4. **Une consommation par domaine d'alimentation**, déduite des six registres : aucune source ne la chiffre, et le
   moniteur n'invente pas une valeur.
5. **Lister les tâches FreeRTOS sans symboles**, en cherchant les structures du noyau en mémoire : non fiable d'une
   version d'ESP-IDF à l'autre ; `FreeRTOS_openocd_params` et l'ELF le permettront (SP4d).
