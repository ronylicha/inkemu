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
   que la vie a écrit. Les règles qui la tiennent :
   - **un journal non appliqué passe avant tout** : quand son application échoue (disque plein, erreur d'E/S), il
     reste et l'image s'en souvient ; avant que la vie suivante lise (`source`) ou sauve (`commit`), il est
     réappliqué, et s'il ne peut toujours pas l'être la vie est refusée (`UnfinishedSave`). Une sauvegarde n'écrit
     jamais par-dessus un journal non appliqué ;
   - **dans l'application, une carte non écrite suit la règle de la flash** (SP2d, R1) : ce que les vies ont écrit
     et que l'image n'a pas pris reste en mémoire, en surcouche posée sur l'image (`host::sd_run::CardKeeper`) ; la
     vie suivante démarre dessus sans relire l'image (la surcouche couvre chaque bloc qu'une sauvegarde interrompue
     aurait changé, donc un `UnfinishedSave` ne la refuse pas), chaque fin de vie et l'extinction réessaient
     d'écrire le tout, et ce qui n'est toujours pas écrit à l'extinction est dit (`NotKept`), avec son remède :
     libérer de la place sur le disque de l'image, ou rendre son dossier inscriptible ;
   - **un journal est lié à son image** : son en-tête (format `inkemu-sd-journal 2`) porte l'identité du fichier
     image, que le condensat couvre : 8 octets qui nomment le fichier, puis sa date de naissance en nanosecondes
     (0 si le système de fichiers n'en garde pas). Sous Unix, l'inode et la date de naissance (`statx`) ; le
     périphérique n'en fait pas partie, car une clé USB rebranchée ou un montage NFS en change, ce qui ferait
     refuser le bon journal. La date de naissance distingue un inode libéré puis redonné à un autre fichier ; là où
     elle manque, l'inode seul décide, et le journal d'une image effacée, laissé à côté d'un nouveau fichier qui a
     repris son inode, lui serait appliqué. Ailleurs (Windows), les 8 premiers octets du SHA-256 du chemin canonique
     et la date de création ; la tunnellisation de NTFS donne à un fichier créé sous le nom d'un fichier effacé moins
     de 15 s plus tôt la date de création de ce dernier : dans cette fenêtre, le journal de l'image effacée serait
     appliqué à la nouvelle. Le journal d'une autre image de même taille, laissé à ce nom, est refusé.
     **Cette identité n'est stable que sur les systèmes de fichiers dont les inodes sont persistants** (ext4, XFS,
     Btrfs, APFS, ZFS…). Sous Linux, FAT/vfat et exFAT, le système habituel d'une clé USB, n'ont pas de numéro
     d'inode sur le disque : le noyau en attribue un par `iunique()` quand il charge le fichier (`fs/fat/inode.c`,
     `fat_build_inode` ; `fs/exfat/inode.c`, `exfat_build_inode` ; `iunique` est, selon `fs/inode.c`, pour « file
     systems that have no natural permanent inode numbering system »), donc un autre numéro après un remontage, un
     redémarrage, ou quand le noyau a oublié le fichier entre deux ouvertures. Leur date de naissance, elle, est
     stockée (vfat et exFAT la rapportent par `statx`). Sur ces systèmes, le journal d'une sauvegarde interrompue
     peut donc être refusé comme étranger (`ForeignJournal`) : le côté sûr, que le message nomme, et la reprise passe
     par `inkemu sd recover --accept-journal`.
     `inkemu sd create` refuse de créer une image là où un journal ou son temporaire existe déjà ;
   - **un refus ne prétend jamais l'image intacte** : un journal ne prend son nom qu'une fois complet, son
     application a donc pu commencer avant l'interruption. Le message d'un journal refusé dit que l'image peut
     contenir une partie de cette sauvegarde, conseille de garder le journal le temps de décider, et dit le risque
     de l'écarter (une image peut-être à moitié écrite, à vérifier par une réparation FAT). Pour une image copiée, ou
     déplacée sur un autre système de fichiers, avec son journal, `inkemu sd recover <image> --accept-journal`
     applique un journal d'une autre identité, toutes les autres vérifications tenues (condensat, capacité, bornes) :
     donné le journal d'une autre image, il écraserait des blocs de celle-ci, ce que la commande dit ;
   - **rien n'est alloué d'après le journal** : sa longueur est comparée avant toute lecture à celle du plus long
     journal de la carte (en-tête de 92 octets, une plage effacée de 16 octets tous les deux blocs au plus, chaque
     bloc écrit une fois en 520 octets) ; il est vérifié puis appliqué en flux, en deux passes, et la sauvegarde
     applique directement les changements validés, sans relire le journal ;
   - **ce qui est ouvert est jugé** : un journal est ouvert sans suivre de lien ni attendre un tube, puis son type est
     revérifié sur le descripteur. En lecture seule, un journal présent est refusé sans être lu, et rien n'est effacé ;
   - **aucune position partagée** : l'image est lue et écrite à des positions données (`read_exact_at` /
     `write_all_at` sous Unix, `seek_read` / `seek_write` sous Windows) ; la source d'une vie et la sauvegarde sont
     deux descripteurs du même fichier, qui partageraient sinon leur position.
   Un **effacement** libère la place où le système de fichiers le permet : Linux perce un trou
   (`fallocate(FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE)`). Sur macOS et Windows, les appels équivalents
   (`F_PUNCHHOLE`, `FSCTL_SET_ZERO_DATA`) n'ont pas d'enveloppe sûre dans nos dépendances et l'espace de travail
   interdit `unsafe` : les zéros sont écrits sur les seules parties qui portent des données (`SEEK_DATA`/`SEEK_HOLE`
   sur macOS et FreeBSD ; sur toute la plage ailleurs, et à partir de l'endroit où le système cesse de répondre, ce
   qui a été trouvé avant étant gardé), et la ligne de fin de vie le dit.
5. **Une image vierge** est formatée par notre code (FAT32, MBR, disposition de l'association SD), déterministe,
   creuse sur les systèmes de fichiers Unix (sur NTFS, un fichier n'est creux que marqué comme tel : `set_len` y
   réserve toute la taille dès la création), écrite sous un nom temporaire en `0600` puis mise à son nom sans
   jamais remplacer ce qui l'aurait pris entre-temps (`renameat2(RENAME_NOREPLACE)` sous Linux glibc, sinon un lien
   dur puis l'effacement du temporaire, dont l'échec est un succès averti, l'image étant en place et le temporaire
   un second nom du même fichier ; sur un système de fichiers sans liens durs, en dernier recours, une vérification
   puis un renommage). La seule crate de formatage FAT répandue (`fatfs`)
   n'a pas de version depuis janvier 2023 : elle ne sert qu'en dépendance de test, pour relire nos images.
6. **Dans une session**, la carte entrera avec le format 2 de SP2d : capacité, protection, plages de blocs non nuls
   compressées (au plus 64 Mio), empreinte de cette forme canonique ; le résultat comparé portera l'empreinte des
   changements de la dernière vie. Un enregistrement dont l'image dépasse la borne est refusé avant l'exécution.

## Conséquences

- Une image de 32 Gio n'occupe en mémoire que ce que la vie écrit ; un effacement de toute la carte est une plage
  dans la surcouche et dans le journal (16 octets).
- Sur le disque, effacer toute la carte libère sa place sous Linux et ne coûte que le percement du trou. Sur macOS,
  il coûte l'écriture de zéros sur ce que l'image contenait (jusqu'à sa taille pleine pour une image remplie), sans
  la faire grossir ; sous Windows, l'écriture de zéros sur toute la plage, dans un fichier qui y occupe déjà toute sa taille.
- Une fin de vie écrit ce qui a changé deux fois (journal, puis en place) ; c'est le prix de l'atomicité.
- Une vie qui suit une sauvegarde inachevée commence par la finir ; tant qu'elle ne le peut pas, la carte ne sert
  pas, plutôt que de lire une image à moitié écrite.
- Copier une image avec son journal, ou la déplacer sur un autre système de fichiers, rend le journal étranger :
  l'utilisateur le fait appliquer par `inkemu sd recover --accept-journal`, à sa parole.
- Un tiers qui écrit l'image pendant une exécution (le verrou est consultatif) rend l'exécution non reproductible ;
  la taille est revérifiée à chaque `commit`, et la session ne dépend jamais du fichier.
- Le format de session change une seule fois pour les boutons et la carte (SP2d, tâche 12).
- **Numérotation** : l'ADR suivant prend le numéro 0015.
