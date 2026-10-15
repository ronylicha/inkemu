# ADR 0014 — La carte SD en SPI, son stockage par blocs et le fichier image de l'hôte

- Date : 2026-10-15
- Statut : accepté (plan SP2e)
- Complète : ADR 0003 (temps de l'appareil), ADR 0004 (fichier de flash), ADR 0007 (sessions), ADR 0012 (signaux, SP2b), ADR 0013 (entrées de la carte, SP2d)

## Contexte

CrossPoint 1.6.5 monte la carte SD avant tout autre écran ; sans carte il affiche « SD card error » et s'arrête là.
Sur le X4, la carte est en mode SPI sur le bus de l'écran (CS GPIO12, MISO GPIO7). La spec globale prévoyait une
crate `storage` pour les « back-ends » SD (image brute, dossier de l'hôte) ; `chips` devait contenir la carte. Une
image de carte se compte en gigaoctets : le schéma du fichier de flash (ADR 0004 : lire le fichier entier au départ,
le remplacer entier par un fichier temporaire renommé à la fin) y coûterait la mémoire de l'image et sa recopie
complète à chaque fin de vie. Les sessions (ADR 0007) portent la flash de départ entière ; une image de 32 Gio ne peut
pas y entrer telle quelle.

## Décision

1. **La carte** (`chips::sdcard::SdCard`) est un modèle au niveau des octets du chapitre 7 de la PLS 9.10 :
   `select`, `exchange`, `transfer`, le temps de l'appareil passé à chaque appel en cycles d'une base donnée à la
   construction. Elle ne connaît pas le SoC ; l'adaptateur `SpiTarget` de SP2d est une boucle sur `transfer`. Elle
   ne voit que ses sélections, n'exige donc pas les coups d'horloge CS haut d'avant CMD0. Ce qu'elle ne modélise pas
   (verrouillage, groupes de protection, CPRM) est refusé en « commande illégale » avec un diagnostic nommé, et son
   CSD ne l'annonce pas.
2. **Le stockage par blocs** est un trait de `chips` (`BlockSource` en lecture, `BlockStorage` en écriture et
   effacement par plage). Trois implémentations sans E/S : `MemoryStorage`, `Overlay` (copie sur écriture d'une
   source, plages effacées gardées comme plages, changements itérés dans l'ordre) et `SparseBlocks` (les blocs non
   nuls d'une session). L'hôte fournit la source de fichier (`host::sd_image::FileBlocks`).
3. **Pas de crate `storage`** pour l'instant : le modèle de stockage est dans `chips` (sans E/S, `wasm32`), le
   fichier et le formatage FAT32 dans `host`. Une crate de plus demanderait de modifier le garde `host_io_ban` de
   `boards`, où SP2d travaille ; la version web (SP6) déplacera le formatage, calcul pur, si elle en a besoin.
4. **Le fichier image est écrit en place, par un journal.** Il est ouvert une fois par exécution et verrouillé sur
   lui-même (exclusif en écriture, partagé en lecture seule : il n'est jamais remplacé, son inode porte le verrou, il
   n'y a pas de fichier `.lock`). À la fin de chaque vie, les changements de la surcouche partent dans
   `.<nom>.journal`, écrit d'abord sous `.<nom>.journal.tmp` en `0600`, vidé sur disque, renommé, puis appliqué en
   place, vidé, et effacé. À l'ouverture, un journal présent est vérifié (SHA-256) et appliqué, ou refusé en le
   nommant ; un temporaire présent est effacé. Après n'importe quelle interruption, l'image est celle d'avant la vie
   ou celle d'après, jamais un mélange : la garantie du fichier temporaire renommé, pour un coût proportionnel à ce
   que la vie a écrit.
5. **Une image vierge** est formatée par notre code (FAT32, MBR, disposition de l'association SD), déterministe,
   creuse, écrite sous un nom temporaire en `0600` puis renommée. La seule crate de formatage FAT répandue (`fatfs`)
   n'a pas de version depuis janvier 2023 : elle ne sert qu'en dépendance de test, pour relire nos images.
6. **Dans une session**, la carte entrera avec le format 2 de SP2d : capacité, protection, plages de blocs non nuls
   compressées (au plus 64 Mio), empreinte de cette forme canonique ; le résultat comparé portera l'empreinte des
   changements de la dernière vie. Un enregistrement dont l'image dépasse la borne est refusé avant l'exécution.

## Conséquences

- Une image de 32 Gio n'occupe en mémoire que ce que la vie écrit ; un effacement de toute la carte est une plage.
- Une fin de vie écrit ce qui a changé deux fois (journal, puis en place) ; c'est le prix de l'atomicité.
- Un tiers qui écrit l'image pendant une exécution (le verrou est consultatif) rend l'exécution non reproductible ;
  la taille est revérifiée à chaque `commit`, et la session ne dépend jamais du fichier.
- Le format de session change une seule fois pour les boutons et la carte (SP2d, tâche 12).
- **Numérotation** : l'ADR suivant prend le numéro 0015.
