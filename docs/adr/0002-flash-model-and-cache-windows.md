# ADR 0002 — Modèle de la flash et fenêtres de cache

- Date : 2026-10-11
- Statut : accepté (plan SP1b-2)

## Contexte

L'ESP32-C3 exécute et lit sa flash à travers deux fenêtres d'adresses de 8 Mio (`0x3C000000` pour les données, `0x42000000` pour les instructions), servies par un cache de 16 Kio qu'une table MMU de 128 entrées, commune aux deux fenêtres, fait correspondre à des pages de flash de 64 Kio. La puce de flash elle-même est pilotée par deux contrôleurs : SPI0 remplit le cache, SPI1 exécute les commandes que le logiciel demande.
Espressif ne documente ni le contrôleur de cache (`EXTMEM`) ni la table MMU dans le manuel de référence technique. Ce que le silicium renvoie pour un accès à travers une entrée invalide, ou quand le cache est désactivé, n'est pas documenté (`docs/research/esp32c3-cache-mmu.md`, sections 6 et 9).
Il faut décider ce que l'émulateur modélise de cet ensemble pour que la ROM et le bootloader s'exécutent sans modification, et ce qu'il fait des cas que rien ne documente.

## Décision

1. **Les fenêtres lisent la flash en direct à travers la table MMU.** Aucune ligne de cache, aucune étiquette, aucun défaut de cache n'est modélisé. Un accès traduit son adresse avec l'entrée de sa page de 64 Kio et lit le contenu de la flash à cet instant.
2. **Le contrôleur de cache se réduit à une logique d'achèvement et à du stockage.** Les opérations que le logiciel attend (invalidation, préchargement, chargement automatique, gel) se terminent aussitôt ; les bits d'activation et de fermeture des bus sont mémorisés et ne sont pas appliqués.
3. **Ce qui ne peut pas être juste en silence est signalé.** Un accès à travers une entrée invalide, ou vers une page située au-delà de la fin de la flash, renvoie 0, verrouille `CACHE_ILG_INT_ST` et `CACHE_MMU_FAULT_*`, et produit un diagnostic nommé ; `--strict-mmio` s'y arrête. Une écriture dans une fenêtre lève une faute d'accès en écriture.
4. **La flash est pour l'instant en lecture seule et appartient au modèle de SPI1**, derrière le trait `FlashDevice`. SPI1 implémente ses commandes câblées et le transfert utilisateur générique ; le modèle de la puce répond aux commandes que la chaîne de démarrage envoie, d'après les mesures, et refuse tout autre opcode en le nommant.
5. **Le code décodé suit la table.** `Bus::page_version` des deux fenêtres change à chaque écriture dans la table MMU ou dans les bits d'activation du cache : le cache de décodage du CPU abandonne ainsi le code de flash qui a pu être déplacé.

## Conséquences

- **ROM et bootloader non modifiés** : la ROM masquée et le bootloader ESP-IDF s'exécutent tels quels. Chacune de leurs attentes se termine, et les quelque 1 300 écritures dans la table que fait le bootloader à chaque démarrage (mesure sur QEMU, `docs/research/esp32c3-bootloader-accesses.md`, §2, ligne 31) ne demandent aucun suivi d'invalidation.
- **Temps** : un défaut du cache de flash ne coûte aucun temps. Le modèle de temps reste « non calibré » ; le coût d'un défaut (spec, section 4.2) revient au plan SP1b-4, par `Bus::fetch_stall`, et sa calibration au sous-projet 8.
- **Écart connu avec le silicium** : un firmware qui réécrit la flash puis relit une zone projetée **sans** invalider le cache verra ici les nouvelles données, et les anciennes sur le silicium. Cela ne compte qu'une fois les écritures en flash disponibles (SP1b-3) ; ce plan devra choisir entre cette vue en direct et des lignes qui restent périmées jusqu'à leur invalidation.
- **Hypothèse** : le comportement d'une entrée invalide est une hypothèse (`ASSUMPTION:` dans `bus.rs`). Le lockstep de SP1b-4, puis un appareil réel, la trancheront. Un firmware sain ne l'atteint jamais : le test du jalon tourne en mode strict.
- **Révision de l'ADR** : à rouvrir quand les écritures en flash arriveront, ou si une mesure sur le matériel contredit le point 3.
