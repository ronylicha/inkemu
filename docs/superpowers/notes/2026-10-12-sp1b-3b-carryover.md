# SP1b-3b — Note de passation

Plan : `docs/superpowers/plans/2026-10-12-sp1b-3b-flash-writes.md` (écritures en flash et fichier de flash de l'appareil). Décisions durables : ADR 0002, révisé (modèle de la flash et fenêtres de cache) et ADR 0004 (entrées d'un run et fichier de flash).

## Résultat

- **La flash s'écrit comme sur l'appareil.** La puce programme (ET bit à bit, repli dans la page de 256 octets), efface un secteur, un bloc de 64 Kio ou tout, et reçoit son registre d'état, sous le verrou d'écriture, qui retombe à la fin de chaque opération. SPI1 transmet l'adresse et les données de ses commandes câblées ; une écriture de `CMD` exécute une commande et une seule. Les six lectures de données répondent.
- **Un firmware compilé depuis ses sources garde un compteur en NVS.** L'exemple `storage/nvs/nvs_rw_value_cxx` d'ESP-IDF v5.5.2, non modifié, vit trois vies en mode strict sur ses deux images : `The value is not initialized yet!`, puis `Restart counter = 1`, puis `= 2`. Les onze programmations sont celles mesurées sous QEMU, dans l'ordre et octet pour octet ; la flash finit comme l'image mesurée (`fc23c7ec…2143f5`, et `6c668205…89de5b` derrière la table du X4).
- **La disposition OTA du X4 démarre en mode strict** : le bootloader efface puis programme `otadata` au premier démarrage, et à celui-là seulement ; les 32 octets sont ceux mesurés, l'effacement et la programmation sont comptés.
- **`inkemu run --flash-file <chemin>` conserve la flash d'un appareil d'un run au suivant.** Un second run sur le même fichier démarre à froid (`rst:0x1 (POWERON)`) et affiche le compteur que le premier a écrit ; le fichier vaut alors l'image mesurée après la deuxième vie (`7ad0acf6…363b709`). `--flash` n'est jamais écrite. Sans fichier, le résumé dit combien de secteurs ont été modifiés et perdus.
- **Déterminisme** : deux runs partis du même contenu ont le même hash de trace et laissent la même flash ; l'image `hello_world` garde son hash de trace `10174cc2726c3d77` (`--max-cycles 40000000`, mode strict).
- **Une lecture de fenêtre qui peut différer du silicium est signalée** (ADR 0002, point 6). Ni la ROM, ni le bootloader, ni ESP-IDF ne déclenchent ce diagnostic sur les images de référence.

## Ce que devient la note de SP1b-3a

Sa section « Reporté au plan SP1b-3b » a été partagée entre cinq plans par la première tâche (tableau « Réattribution du 12 octobre 2026 » en tête de cette note-là). Ce que ce plan en a pris est fait :

- écritures en flash, lectures de données par SPI1, adresse et données des commandes câblées, mise à jour d'`otadata` comptée dans le test OTA ;
- contenu rendu par la machine et état « modifié » (`Esp32C3::flash_contents`, `flash_modified_sectors`), fichier de flash, troisième terme de `Bus::page_version` ;
- le choix laissé ouvert par l'ADR 0002 : la vue en direct est gardée, et la lecture qui peut différer est nommée ;
- la règle de taille d'une `FlashDevice` venue de l'extérieur (note SP1b-2) : `SocBus::new` et `Peripherals::new` ne sont plus publics, une puce n'est construite que depuis `SocConfig::flash` ;
- le commentaire faux de `Spi1::run` et le test mal nommé (`USR | FLASH_PE` n'est pas l'encodage d'une programmation d'ESP-IDF : la programmation mesurée est `CMD = 0x02000000`, `ADDR = longueur << 24 | adresse`) ;
- l'hypothèse 13 de la note SP1b-2 (adresse d'un transfert utilisateur non décalée) : elle est sourcée pour 24 bits et n'est plus une hypothèse.

## Décisions prises pendant l'écriture du plan

Elles sont dans le tableau des décisions du plan, à partir de G19. Les principales :

- Plusieurs bits de commande dans une même écriture de `SPI1.CMD` sont refusés en les nommant, plutôt qu'exécutés dans un ordre deviné.
- SPI1 tient un journal borné (1 024 entrées, le surplus compté) de ce que la puce a écrit, et compte par opcode les commandes qu'il lui envoie : c'est ce que les tests comparent aux mesures.
- « Secteurs modifiés » et la génération du contenu ne comptent que les octets réellement changés : une programmation qui laisse chaque octet tel quel ne modifie rien, n'invalide rien et ne fait rien écrire.
- Les refus de la puce sont quatre diagnostics nommés (opcode inconnu, trame inconnue, hors de la puce, protégé) ; une écriture sans verrou est ignorée en silence, comme sur la pièce.
- Le fichier de flash est verrouillé par un fichier voisin `<chemin>.lock`, jamais supprimé.
- Un fichier de flash d'une autre taille est refusé même sous `--reset-flash` : une image de firmware donnée par erreur à `--flash-file` doit survivre.
- Un fichier de flash impossible à écrire à la fin est une erreur de code 2, sauf après un arrêt de `--strict-mmio`, qui garde 3 ; aucun nouveau code de sortie.
- Le SHA-256 du contenu de départ est imprimé à chaque run, avec ou sans fichier de flash (12 ms pour 16 Mio en version optimisée, mesuré).
- Les tests du firmware de référence sont sélectionnés par le fragment de nom `reference_firmware` dans la CI.
- Les deux exemples de documentation de `soc-esp32c3` (`SocBus::new`, `Esp32C3::flash_contents`) reçoivent leur `RomImage` en paramètre. `RomImage::from_flat` n'existe que sous la fonctionnalité `test-support` : un exemple qui l'appelait se compilait avec `cargo test --workspace`, où un autre crate active la fonctionnalité, et échouait avec `cargo test -p soc-esp32c3`. Pour l'exemple marqué `compile_fail`, c'était pire qu'un échec : il réussissait pour une mauvaise raison, quelle que soit la visibilité de `SocBus::new`. Un exemple `compile_fail` ne prouve que ce que sa mutation fait échouer.

## Décisions prises pendant l'exécution

Aucune : le plan a été exécuté tel qu'il est écrit.

## Ce que les tests prouvent, et ce qu'ils ne prouvent pas

Chaque comportement que le plan affirme nomme la mutation qui doit faire échouer son test ; elles ont été essayées. Quatre mutations survivent aux tests du firmware de référence, et sont tuées ailleurs :

- **ET ou écrasement.** NVS ne programme que des valeurs qui effacent des bits : le firmware de référence ne distingue pas une puce qui fait `ancien ET nouveau` d'une puce qui écrase. La règle est prouvée par le test unitaire `a_program_only_clears_bits`.
- **Un effacement qui n'efface rien.** Le secteur d'`otadata` que le bootloader efface est déjà effacé dans les images. L'effacement est prouvé par les tests unitaires de la puce et, à travers la commande, par le compteur de démarrages de `the_flash_file_keeps_the_flash_of_the_device_from_one_run_to_the_next`.
- **Le code décodé.** Aucun firmware de référence n'exécute du code qu'il vient de réécrire : le troisième terme de `page_version` est prouvé par des tests de la machine.
- **L'invalidation d'une plage.** Les images de référence ne dépendent que de l'invalidation de tout le cache (au démarrage, et par `bootloader_mmap`) ; l'invalidation par plage de `spi_flash_check_and_flush_cache` n'est couverte que par des tests unitaires.

Couverts par des tests unitaires seulement, faute d'un firmware de référence qui les exerce : un effacement demandé par une application, l'effacement de bloc `D8` (`CMD = 0x00800000`), les programmations de 64 octets et leur découpe à la frontière d'une page, `otadata` écrit par une application, le changement de page de NVS (il faut environ 125 vies par page).

Non testable : le vidage sur le disque avant le renommage du fichier de flash (`sync_all`). Les refus du script de construction du firmware quand une empreinte ne correspond pas n'ont pas été rejoués dans Docker ; l'étape de vérification de la CI (`sha256sum -c`) l'a été sur une copie altérée.

## Hypothèses nouvelles

Quatre marqueurs `ASSUMPTION:` s'ajoutent, un disparaît : `grep -rn "ASSUMPTION:" crates --include=*.rs` rend 80 lignes (77 à la fin de SP1b-3a), et les profils gardent leurs quatre marqueurs.

- `flash.rs`, `NorFlash::read_data` : les lectures quadruples (`6B`, `EB`) répondent quel que soit le bit QE du registre d'état. (Une image dont l'en-tête dit QIO ou QOUT, lue contre un appareil.)
- `flash.rs`, `NorFlash::write_status` : `SRWD` est mémorisé et ne verrouille rien. (Le niveau de la broche `WP#` : le modèle de broches de SP2, ou un schéma.)
- `periph/spi_mem.rs`, `Spi1::run_hard_wired` : la lecture câblée `FLASH_READ` envoie `ADDR[23:0]` et lit `MISO_DLEN` octets dans `W0..`. Ni la ROM ni ESP-IDF v5.5.2 n'écrivent ce bit. (Un firmware qui s'en sert, lu contre un appareil.)
- `periph/extmem.rs`, `Extmem::invalidate` : une invalidation de taille nulle à une adresse non nulle, ou à une adresse hors des deux fenêtres, ne retire rien. (Lire une fenêtre, la programmer, émettre une telle demande et relire, sur un appareil.)
- Retirée : l'adresse d'un transfert utilisateur prise non décalée dans `ADDR` (mesurée pour 24 bits : FW §A.1 et §P, SF §C).

## Reporté, par héritier

- **SP1b-3c** : `--firmware` (un `.bin` d'application seul), l'ADR sur le bootloader embarqué qui doit le précéder (il prend le numéro 0005), les tables de partitions dans les profils, les retouches de forme de `boards` (`# Errors` de `from_named_toml`, `ProfileError::Parse`).
- **SP1b-3d** : réception et interruptions UART, format de trame (8 lignes de la table), côté hôte de l'USB-Serial-JTAG, pilote de console commun aux tests d'intégration et leur accès à `test_support`.
- **SP1b-3e** : le démarrage d'un firmware communautaire (CrossPoint).
- **SP1b-3f** : sommeil profond et réveil par le timer RTC (10 lignes d'alimentation de la table), les bits 29:27 de `RTC_CNTL.TIME_UPDATE`, le découpage de `periph/timg.rs` et de `periph/rtc_cntl.rs` ; le sommeil léger y est refusé en le nommant.
- **SP1b-4** : un cache fidèle à la ligne (ADR 0002 : limites du diagnostic, et lecture après un changement d'entrée MMU sans invalidation) avec le coût d'un défaut ; l'enregistrement et le rejeu, qui portent les entrées d'un run (ADR 0004) ; les hypothèses ci-dessus ; les 28 lignes de la table qui le nomment.
- **SP2** : l'appareil virtuel doté d'une identité et l'endroit où ranger son fichier de flash ; une sauvegarde de la flash en cours de run si le besoin se confirme ; le niveau de `WP#` ; l'identifiant de `DeviceProfile` comme type validé.
- **SP8** : les durées réelles des opérations de flash (le bit d'occupation ne se lève jamais : le premier démarrage d'une disposition OTA est plus court ici que sur l'appareil d'une centaine de millisecondes), l'attente automatique et la suspension (`SPI1.FLASH_WAITI_CTRL`, `FLASH_SUS_CTRL`, `SUS_STATUS` : la demande de suspension est refusée en la nommant), la carte de protection par blocs d'une pièce réelle (un programme ou un effacement sous un bit BP est refusé en le nommant). Le registre d'état n'est pas dans le fichier de flash : ses bits non volatils reviennent à leur valeur d'usine à chaque run.
- **SP9** : Windows. Les tests du verrou et du répertoire disparu supposent un système de fichiers POSIX ; `File::try_lock` existe sur Windows mais n'y a pas été essayé.
- **Aucun héritier** : les registres de programmation des eFuse restent des accès non modélisés, signalés par leur nom. Un fichier temporaire orphelin (`.<nom>.<pid>.tmp`) laissé par un processus tué pendant l'écriture finale n'est supprimé par aucun run.

## Lignes de la table des registres acceptés, par héritier

SP1b-3d : 8. SP1b-3f : 10. SP1b-4 : 28. SP2 : 26. SP5a : 8. SP5b : 2. SP8 : 22. Plus aucune ligne ne nomme SP1b-3b. Les trois lignes d'invalidation d'`EXTMEM` (`ICACHE_SYNC_CTRL`, `_ADDR`, `_SIZE`) disent désormais leur seul effet : mettre fin au diagnostic de lecture périmée des pages qu'elles couvrent.

## Coût en CI

Mesuré sur la machine de développement, pas sur un runner GitHub :

- construction du firmware de référence, seulement quand le cache est manqué : 34 s pour `hello_world` et 32 s de plus pour `nvs_rw_value_cxx` dans le même conteneur (le script imprime ces durées dans le journal du job) ;
- job `reference-boot` : `reference_boot` passe de 7 à 12 tests (2,7 s à 8,4 s), les tests de la commande de 2 à 3 (4,8 s) ;
- job `rom-boot` : les tests de la commande passent de 10 à 16 (2,1 s).

Le temps de compilation des jobs ne change pas : aucune dépendance n'est ajoutée.

## À reporter dans la spec

- **§5.1, persistance** : « chaque appareil virtuel possède son fichier flash » devient, dans la CLI, un fichier demandé par `--flash-file` ; sans l'option rien n'est conservé. Le fichier est écrit à la fin du run, pas au fil de l'eau : un run tué perd ses écritures. La « commande de remise à zéro explicite » est l'option `--reset-flash`. (ADR 0004.)
- **§5.1, entrées acceptées** : un dump complet donné à `--flash` n'est jamais écrit ; c'est le fichier de `--flash-file` qui porte l'état.
- **§3.1, déterminisme** : le contenu de la flash au départ est une entrée d'un run, nommée par son SHA-256 ; le hash de trace seul ne distingue pas deux runs partis de deux contenus quand le firmware suit le même chemin. (ADR 0004.)
- **§7, erreurs** : un fichier de flash impossible à écrire à la fin donne le code de sortie 2 (3 après un arrêt strict).
- **`--max-time`** : toujours absent de la spec (déjà signalé par la note SP1b-3a).
