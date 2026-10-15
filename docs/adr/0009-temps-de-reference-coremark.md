# ADR 0009 — Le temps de référence : CoreMark et le critère 5

- Date : 2026-10-15
- Statut : accepté (plan SP1b-4e)

## Contexte

La spec globale veut un modèle de temps « cycle-approché », « recalé sur des références publiques mesurées (score
CoreMark publié de l'ESP32-C3) » (§1 point 2), dont l'interface dit toujours s'il est « non calibré sur matériel » ou
« calibré (appareil, date) » (§1 point 4), et un test qui vérifie que CoreMark émulé reste dans ±15 % du score publié
(§4.2, §10.3 critère 5). Le modèle livré, `DatasheetTiming` (`crates/cpu-rv32/src/timing.rs`), donne à chaque
classe d'instruction un coût estimé, sans mesure ; la mémoire n'ajoute aucun état d'attente ; il se déclare non
calibré (`is_calibrated() == false`, ligne `timing model datasheet-uncalibrated (not calibrated)`).

Deux chiffres sont publiés (rapport `docs/research/coremark-esp32c3.md`, CM ci-dessous, §1) : la fiche technique
v2.4 donne 483,27 pour un cœur à 160 MHz, sans compilateur ni options ; l'exemple `espressif/coremark` imprime
409,25 avec GCC 12.2. Le firmware que construit le projet (GCC 14.2, `-O3`, code en IRAM) imprime sous inkemu
424,628450, chronométré par `esp_timer`, c'est-à-dire sur le temps d'appareil d'inkemu (CM §4).

## Décision

1. **Le score de référence est celui de la fiche technique, 483,27** ; la bande du critère 5 est [410,78 ;
   555,76]. Le chiffre de l'exemple est rapporté à côté, jamais testé. La mesure tombant dans les deux bandes, le
   choix ne dépend pas d'elle.
2. **Le score tenu est celui que le firmware imprime avec le modèle de temps par défaut**, sur le temps d'appareil
   d'inkemu. Aucun modèle de temps n'est propre au test : ce que la CI tient est ce que l'utilisateur fait tourner.
3. **Aucun coût n'est ajusté sur CoreMark tant qu'il est dans la bande.** Un benchmark donne une équation pour dix
   coûts de classe : il borne une moyenne, pas un modèle. Sur le mélange de CoreMark, la bande admet tout modèle dont
   le coût moyen est entre 0,97 et 1,31 cycle par instruction (1,264 aujourd'hui) ; un modèle à un cycle par
   instruction y entre aussi (environ 537). Un ajustement choisirait sans preuve un modèle parmi une infinité, sur un
   chiffre dont le compilateur n'est pas connu, et déplacerait toutes les empreintes de trace épinglées.
4. **Un score qui sort de la bande est un défaut du modèle de temps**, cherché et corrigé dans le modèle, avec un
   test ; la bande n'est jamais élargie ni la référence changée pour faire passer une exécution.
5. **« Calibré » reste réservé à une mesure sur un appareil** (§1 point 4) : tenir le critère 5 ne fait pas passer
   `DatasheetTiming` à `is_calibrated() == true`. Le banc du sous-projet 8 ajustera les coûts sur une puce ; le
   modèle qu'il produira devra tenir le critère 5 lui aussi, et si sa mesure contredit la fiche technique, c'est une
   nouvelle révision de cet ADR qui changera la référence, avec sa raison.

## Conséquences

- Le job `reference-boot` construit `coremark_example.bin` (sources d'Espressif copiées sans modification, vérifiées
  contre le registre) et exécute un test qui échoue hors de la bande : vérifié par une mutation (coût ALU doublé :
  295,53, −38,8 %, le test échoue).
- Le test vérifie les deux bornes, mais la borne haute (555,76) est hors d'atteinte d'un modèle qui coûte au
  moins un cycle par instruction : un cycle pour chacune donne environ 537. Seule la borne basse peut donc
  faire échouer un modèle de cœur à une instruction par cycle ; un modèle trop rapide d'au plus 15 % passerait,
  seul un appareil le dirait. Les coûts relatifs des classes (branchement pris, division…) ne sont pas
  vérifiés par CoreMark.
- La ligne de l'erreur standard qui nomme le modèle ne change pas : `datasheet-uncalibrated (not calibrated)`.
- Le score qu'imprime CoreMark sous inkemu n'est jamais présenté comme un score CoreMark d'un appareil (accord
  d'usage de la marque, CM §2).

## Alternatives écartées

- **Le chiffre de l'exemple (409,25)** comme référence : une sortie d'exemple, d'un compilateur plus ancien que celui
  du projet ; la fiche technique est le score de la puce.
- **Exiger les deux bandes** (leur intersection, [410,78 ; 470,64]) : plus stricte, mais elle ferait échouer un modèle
  calibré sur une puce qui donnerait le chiffre de la fiche technique (483,27 est hors de la bande de l'exemple).
- **Ajuster un ou deux coûts** (branchement pris, latence de chargement) pour viser 483 : voir la décision 3.
- **Un modèle de temps propre au test** : la CI tiendrait un modèle que l'utilisateur ne fait pas tourner.
