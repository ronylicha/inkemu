# SP1b-1 — décisions prises et points reportés

Bilan de l'exécution du plan `docs/superpowers/plans/2026-10-11-sp1b-1-rom-banner.md` : les 12 tâches sont livrées, chacune relue, puis la branche a été relue en entier et une vague unique de corrections (21 entrées) a été appliquée. Ce document garde deux choses que l'historique git ne dit pas : les **décisions** prises en cours de route, et ce qui est **volontairement reporté**, avec le plan qui en hérite.

## Résultat

`inkemu run --device x4` exécute la vraie ROM masquée de l'ESP32-C3 depuis son vecteur de reset et affiche sa bannière. En mode strict, les trois révisions de ROM (rev0, rev3, rev101) atteignent leur bannière sans aucun accès à un registre non modélisé et sans exception CPU. Après la bannière, la ROM suit son chemin de démarrage sur flash : chaque registre non modélisé apparaît comme avertissement nommé, et l'exécution finit par attendre le contrôleur de cache (`EXTMEM.ICACHE_SYNC_CTRL`). C'est le point de départ du plan SP1b-2.

## Décisions prises pendant l'exécution

| # | Décision | Raison | Coût si elle est fausse |
|---|---|---|---|
| R1 | Un test épingle le format du fichier généré par `svd-gen`. | Ce format est un contrat entre le générateur et `svd-regs`. | Nul. |
| R2 | Une ligne de test sans assertion a été retirée du plan. | Elle ne vérifiait rien. | Nul. |
| R3 | La conversion de l'ELF de ROM borne son travail (128 segments, 512 sections) et refuse un segment qui déborde de la ROM. Les chemins d'erreur sur fichier hostile sont testés un par un. | Un ELF est une entrée non fiable ; la spec (§7) interdit un état silencieusement faux. | Une future ROM au-delà de ces bornes demandera de relever deux constantes : le message le dit. |
| R4 | Le miroir de lecture des eFuse ne répond qu'aux 91 registres que le SVD documente ; les 12 adresses réservées de sa fenêtre produisent un avertissement. | Le plan demandait de répondre zéro partout, ce qui contredisait la spec (§5.2). | Une lecture d'une adresse réservée avertit au lieu de rendre zéro en silence. |
| R5 | **La machine ESP32-C3 exécute les accès mémoire non alignés** au lieu de lever une exception. | La ROM rev0 fait une lecture non alignée dans `_init` avant qu'un gestionnaire d'exception puisse l'émuler : le silicium les exécute. Détail et source : `docs/research/esp32c3-boot-chain.md`, §M.1. | Un firmware qui compterait sur cette exception ne serait pas reproduit ; le lockstep de SP1b-4 le montrerait. Le banc de conformité (`inkemu rvtest`) continue de lever l'exception par défaut. |
| R6 | Le répertoire de données de l'utilisateur est résolu par `etcetera` (MIT/Apache) et non `directories`. | `directories` tire une dépendance sous MPL-2.0 ; la politique de licences reste « permissives uniquement ». | Le chemin du cache suit les conventions d'`etcetera` : à vérifier sur macOS et Windows au sous-projet 9. |
| R7 | Le tri de la revue finale est accepté tel quel : 48 remarques mineures corrigées avant fusion, 2 reportées, 4 écartées. | Tout ce qui rendait une affirmation fausse, un échec silencieux ou une entrée dangereuse a été corrigé. | Les 4 remarques écartées sont cosmétiques. |
| R8 | Après la relecture finale, un correctif supplémentaire a rendu vrais quatre messages : remèdes `rom fetch` exécutables, annonce « downloading » émise seulement quand un téléchargement démarre, cause ajoutée au message `--device`/`--all`, README corrigé. | La règle « une seule vague de corrections » cède devant un message faux montré à l'utilisateur. | Nul. |
| R9 | La commande imprimée dans un remède nomme le répertoire que l'utilisateur a choisi (y compris pour `rom fetch --rom-dir`), et l'écrit nu, entre guillemets doubles, ou sous la forme `<directory>` quand aucun échappement n'est valable à la fois dans un shell POSIX, cmd.exe et PowerShell. | Une commande qui télécharge ailleurs que promis, ou qu'un espace coupe en deux, n'est pas un remède. | Un chemin au caractère exotique reçoit `<directory>` au lieu d'une commande à coller ; le message nomme toujours le chemin. |

Les décisions de conception D1 à D12 du plan restent valables, sauf D10 complétée par R5 et le choix de `directories` (Tech Stack) remplacé par R6.

## Reporté au plan SP1b-2 (flash, cache/MMU, SHA, bootloader)

- Premier registre attendu par la ROM après sa bannière : `EXTMEM.ICACHE_SYNC_CTRL` (elle boucle sur son bit de fin). Viennent avant lui `GPIO.ENABLE`, la matrice GPIO, `IO_MUX` des broches de flash, SPI0/SPI1 : voir la liste des avertissements de `inkemu run --device x4` et le rapport `esp32c3-boot-chain.md`, §J lignes 13 à 23.
- La table MMU (`0x600C_5000`) n'est pas dans le SVD : à modéliser à la main.
- eFuse : adresse MAC et version de bloc (le bootloader les lit), registres de programmation.
- `SYSTEM.CPU_PER_CONF` et l'arbre d'horloges (passage du CPU à 80 puis 160 MHz).
- Profils d'appareil : taille de flash, tables de partitions, puis validation d'un profil fourni par l'utilisateur (identifiant vide, cohérence de l'identifiant). Si `--device <fichier.toml>` voit le jour, nettoyer le message d'erreur de `toml` avant de l'afficher (il cite la ligne du fichier).
- Points de SP1a toujours ouverts : `fetch_stall` et le cache flash, l'heuristique de dump flash, le message de `BadFlashSize`, le `Display` de `SpiMode`.
- `rom_store` décompresse l'archive une fois par ELF manquant (trois fois pour `rom fetch --all` à froid) : une seule passe suffirait.
- Trois modules de tests écrivent des adresses de base en clair au lieu de les prendre dans `svd-regs`.

## Reporté au plan SP1b-3 (interruptions, timers, watchdogs, reset)

- La matrice d'interruptions mémorise sa configuration mais ne livre rien au CPU. Le cœur ne doit pas dépendre de `mie` pour les lignes du C3 (rapport §B.1).
- **Temps des périphériques** : l'UART émet instantanément, donc le temps de démarrage émulé est sous-estimé. Décider comment un périphérique apprend l'heure (argument de contexte, ou état posé par le bus avant chaque pas, comme pour le PC) avant d'écrire les timers : onze implémentations de `Peripheral` existent déjà. Le contrat `Peripheral` recevra alors son `tick`.
- Registres acceptés sans effet aujourd'hui, à rendre réels : `UART.INT_ENA` et `INT_CLR`, `RTC_CNTL.SWD_CONF` (drapeau en lecture seule et impulsions), `INTERRUPT_CORE0.CPU_INT_CLEAR`, `ASSIST_DEBUG` (PC enregistré au reset).
- Reset logiciel (`RTC_CNTL.OPTIONS0`), causes de reset, watchdogs réellement actifs (critère 4 de la spec).
- Réception sur UART et USB-Serial-JTAG, branchement et débranchement de l'hôte USB.
- Journal d'une exception CPU : la spec (§7) demande le PC et la pile ; le diagnostic porte le PC et `tval`.
- Points de SP1a toujours ouverts : `DecodeCache::clear()` et son `Debug` manuel, latence d'interruption, PMP et déclencheurs.

## Reporté au plan SP1b-4 (lockstep QEMU, CoreMark, rejeu, hygiène CI)

- Hypothèses marquées `ASSUMPTION:` dans le code, à trancher par le lockstep : coût en cycles d'un accès non aligné ; fetch à travers une adresse du bus de données ; compteur de performance avec plusieurs événements sélectionnés ; bits conservés par `ustatus`.
- Accès non aligné à un registre de périphérique : erreur de bus aujourd'hui, comportement réel inconnu.
- Empreinte de trace : elle ne mélange que les PC. En faire une empreinte d'état (écritures de registres, de CSR, de MMIO) la rendrait plus forte et servirait le lockstep.
- CI : clé de cache du job `rom-boot` statique (une entrée corrompue fait échouer le job jusqu'à sa purge : il échoue fermé), couverture contrôlée par crate, épinglages des actions.

## À prévoir plus tard

- **SP2** : le manifeste de ROM, la règle de choix de révision et les deux contrôles d'empreinte ne font aucune entrée-sortie ; l'application de bureau et la version web en auront besoin sans `ureq` ni `std::fs`. Scinder `rom_store` en une partie sans E/S et une partie cache/téléchargement.
- **SP9** : vérifier le chemin du cache de ROM sur macOS et Windows (R6).
- La révision de silicium réelle des Xteink X4 et X3 reste inconnue ; les profils supposent v0.3. `esptool.py chip_id` sur un appareil, ou la ligne « chip revision » de son journal de démarrage, trancherait.
