# SP4 — découpage décidé le 2026-10-15

Source : `docs/research/sp4-debugging.md` (ce que la machine donne déjà, protocole GDB, désassemblage, matériel de
débogage de l'ESP32-C3, tâches FreeRTOS et tas, scénarios d'autres émulateurs). Spec globale : §6.1 (moniteur de
ressources), §6.2 (débogage), §6.4 (CLI et tests automatisés), feuille de route ligne 4 (« GDB, moniteur, scripts,
régression visuelle »). Point de départ : `main` à `6691a7d` (SP2 fait, CrossPoint 1.6.5 sur son écran d'accueil
dans l'application). Règle de l'utilisateur : « interface d'abord », la logique dans des crates bibliothèques, la
CLI mince. Décisions prises sans l'utilisateur, à lui restituer au bilan ; elles sont réversibles plan par plan.

## Plans, par valeur pour l'utilisateur

Ordre : d'abord ce qui rend l'application plus utile à qui met au point un firmware, puis ce qui branche des outils
extérieurs, puis ce qui tourne sans écran, enfin le matériel de débogage propre à la puce.

| Plan | Livrable utilisable | Hérite | Taille |
|---|---|---|---|
| **SP4a** | **Le débogueur dans l'application** : pause, pas à pas (existent), registres x0–x31 par leurs noms ABI, CSR, mode du processeur, désassemblage RV32IMC autour du PC, vue de la mémoire lue sans effet, points d'arrêt sur une adresse d'instruction, points de surveillance sur une adresse (lecture, écriture, accès) ; un arrêt ne change rien à l'exécution (empreinte de trace et verdict de rejeu identiques) ; l'état final d'une vie reste lisible jusqu'à la suivante | l'API de pas à pas et d'introspection de SP1b-4c | M |
| SP4b | **Le moniteur de ressources** (spec §6.1) : fréquence, charge (part du temps d'appareil hors `wfi`), rapport de vitesse, occupation et plus haut niveau de chaque région de RAM, batterie et câble ; consommation datasheet selon l'état d'alimentation, signalée comme telle ; les registres d'alimentation acceptés sans effet faute de modèle | 6 lignes de la table des registres (`RTC_CNTL.OPTIONS0`, `PWC`, `DIG_PWC` hors `DG_WRAP_PD_EN`, `BIAS_CONF`, `SDIO_CONF`, `SYSTEM.MEM_PD_MASK`), consommation pendant le sommeil profond | M |
| SP4c | **L'appareil vivant et la vue technique de l'écran** : brancher ou débrancher le câble, faire varier la batterie, insérer, retirer ou protéger la carte pendant une vie (entrées journalisées, format de session si besoin) ; RAM brute du SSD1677, compteur de rafraîchissements partiels depuis le dernier complet, durée de chaque rafraîchissement, animation trame par trame ; persistance de l'image de la dalle d'une exécution à l'autre ; installer une application en gardant la NVS ; profils de l'utilisateur | RAM brute, compteur de partiels (SP2a/SP2c/SP2d) ; câble, batterie et carte pendant une vie (SP2d, SP2e) ; **borne de la mémoire des changements de carte non écrits** (ADR 0014, R1/N2 de SP2d) ; persistance de la dalle (SP2a) ; animation (SP2a) ; installation en gardant la NVS, profils (SP2c) | L |
| SP4d | **Symboles ELF et serveur GDB** : un `.elf` associé au `.bin` (spec §6.2) donne les symboles au désassemblage et à la pile d'appels, et les tâches FreeRTOS par `FreeRTOS_openocd_params` (ESP-IDF ≥ 5.0) ; un serveur RSP (`inkemu gdb --port 3333`, interrupteur dans l'application) sur les arrêts de SP4a, pour gdb et VS Code ; `inspect` depuis l'interface | les arrêts et la lecture de SP4a ; `inspect` (SP2c) | L |
| SP4e | **Scénarios sans écran et régression visuelle** (spec §6.4) : format YAML (boutons, gestes, attentes conditionnelles : écran stable, texte sur une console, délai virtuel ; captures et comparaisons) qui remplace `inkemu-console-script 1` ; code de sortie et rapport ; un scénario produit une session ; lancé depuis la CLI et depuis l'application | critère de réussite d'une exécution scriptée ; remplacement du format de script (ADR 0006) ; scénario → session (ADR 0007) ; scripts de console depuis l'interface ; vidage de la console USB-Serial-JTAG jamais ouverte par un hôte ; SIGTERM et le code 143 | L |
| SP4f | **Le matériel de débogage de la puce** (celui du firmware) : déclencheurs (`tselect`, `tdata1`/`mcontrol`, `tdata2`, `tcontrol`, 8 unités, TRM §1.7), `rv32mi-p-breakpoint` sorti de l'exclusion de la conformité, garde de pile et moniteurs de région d'`ASSIST_DEBUG` (sources 54), permissions `SENSITIVE`/PMS (sources 55–60) | reports de SP1b-4f | M |

## Arbitrages

- **Le débogueur de l'émulateur n'est pas celui de la puce.** L'émulateur arrête la machine de l'extérieur, sans
  déclencheur, à une frontière d'instruction : le firmware n'en voit rien, et l'exécution ne change pas. Les
  déclencheurs de l'ESP32-C3 sont des fonctions du firmware (la garde de pile d'ESP-IDF, `esp_cpu_set_watchpoint`) :
  ils ont leur propre plan (SP4f) et leur test de conformité.
- **SP4a d'abord** parce que tout le reste s'appuie sur ses arrêts et sa lecture sans effet : le serveur GDB (SP4d)
  est le même débogueur piloté par un autre client, le moniteur (SP4b) lit les mêmes registres, un scénario (SP4e)
  peut s'arrêter sur un point.
- **Les symboles vont avec GDB** (SP4d) : tous deux lisent l'ELF ; les tâches FreeRTOS et le tas en dépendent
  (spec §6.1 : le tas « seulement si ses structures ESP-IDF sont reconnues »).
- **La borne mémoire des changements de carte non écrits** (ADR 0014) va avec la vie de la carte (SP4c) : insérer ou
  retirer une carte en cours de vie change déjà qui tient ces changements.
- **Le contenu de la flash** : la règle du dépôt (aucun octet de flash affiché, la NVS d'un appareil réel pouvant
  contenir des données personnelles) vaut pour tout le sous-projet. SP4a la tient par une politique écrite
  (ADR 0016) ; SP4d la reprend pour les sections de l'ELF que l'utilisateur fournit.
- **Écrire un registre ou la mémoire** depuis le débogueur changerait l'exécution sans entrer au journal : hors de
  SP4a ; SP4d tranchera pour les paquets `G`, `P` et `M` de GDB (refus pendant un enregistrement, ou entrée de
  journal nouvelle).
- **Hors de SP4, nommés** : instantané d'état pour reprendre au milieu d'une exécution (non demandé par la spec) ;
  SDMMC (SP5) ; dossier de l'hôte en FAT32 virtuel (après la livraison du 2027-01-01).
