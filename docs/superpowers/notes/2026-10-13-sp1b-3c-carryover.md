# SP1b-3c — Note de passation

Plan : `docs/superpowers/plans/2026-10-13-sp1b-3c-firmware.md` (`--firmware` : une image d'application démarre derrière le bootloader qu'inkemu embarque). Décision durable : ADR 0005 (le bootloader de second étage, embarqué, choisi par le profil, remplaçable).

## Résultat

- **`inkemu run --device x4 --firmware hello_world.bin` démarre l'application seule**, derrière le bootloader ESP-IDF v5.5.2 qu'inkemu embarque et la table de partitions d'origine du X4 : en mode strict, toute la vie de l'application, son redémarrage et le second démarrage, sans accès non modélisé. Au premier démarrage, le bootloader imprime ses 26 lignes, celles de la capture QEMU de la même flash (`golden_hello_ota_boot.log`) aux deux différences connues près (révision de bloc eFuse v1.2, mode SPI DIO), dont la table du X4, `No factory image, trying OTA 0` et `Set actual ota_seq=1 in otadata[0]` ; au second, ni l'une ni l'autre de ces deux lignes.
- **La flash composée est celle de la construction de référence** : pour `hello_world`, `9ddacb60…11bc89`, l'image `flash_hello_ota.bin` du rapport TO §2 ; pour `nvs_rw_value_cxx`, `87832c6d…f28fec`, l'image épinglée `flash_nvs_ota.bin`. Le SHA-256 est imprimé (`flash at start`), et deux exécutions ont la même empreinte de trace : `c6b24a1cc226ed4a` pour `--firmware hello_world.bin --strict-mmio --max-cycles 40000001`, 11 403 054 µs d'appareil, 26 637 404 instructions, épinglée par un test. L'empreinte de la vie de l'image fusionnée, `10174cc2726c3d77`, ne change pas.
- **Le compteur de NVS continue d'une exécution à l'autre à partir de l'application seule** : une première exécution `--firmware nvs_rw_value_cxx.bin --flash-file x4.bin` écrit `otadata` et affiche `The value is not initialized yet!` ; une seconde, avec le fichier seul, démarre à froid et affiche `Restart counter = 1`.
- **Chaque refus est nommé avant le démarrage**, avec la cause puis le remède, et testé à travers la commande : un fichier qui n'est pas une image, un `.xota` chiffré, une image ESP32-S3 pour un X4, une application plus grande que son emplacement, une empreinte fausse, une image entière (« `--flash` »), un bootloader donné comme firmware (« `--bootloader` »), une application donnée comme bootloader (« `--firmware` »), un bootloader qui demande une puce plus récente (il la réinitialiserait sans fin).
- **`inkemu licenses`** imprime le `NOTICE` d'inkemu puis l'avis et la licence de chaque élément tiers qu'inkemu embarque comme données : les descriptions de registres et le bootloader. L'avis du bootloader dit aussi la seule modification qu'inkemu lui apporte, la réécriture de son en-tête pour une flash qui ne fait pas 16 Mio (licence Apache, section 4(b)). Le même texte vient de `boards::notices`, pour la page « À propos » d'une interface. Les avis des bibliothèques Rust liées dans l'exécutable n'y sont pas encore (voir SP9 plus bas), et la commande ne se dit pas complète.
- **Un nouvel appareil est un fichier et aucun code** : `crates/boards/profiles/<id>.toml`. Le script de construction de `boards` dresse la liste des profils livrés à partir de ce répertoire, triée ; un test compare cette liste aux fichiers. Une nouvelle famille de puce est une entrée de `firmware::MEMORY_MAPS` (où sa ROM charge un bootloader), une entrée de `firmware::SHIPPED` et son répertoire dans `third_party/esp-idf-bootloader/`.
- **Tout passe par des bibliothèques** : composition, registre des bootloaders, contrôles et tables de partitions sont dans `firmware` et `boards`, avec des erreurs typées. `emu-cli` lit les fichiers, appelle `DeviceProfile::compose_flash` et imprime. Les entrées-sorties de l'hôte (fichiers, environnement, horloges, processus, réseau) sont refusées par clippy dans `firmware`, `boards` et `soc-esp32c3`, par le `clippy.toml` de chacun de ces crates ; l'étape WebAssembly de la CI ne prouve que la compilation de ces crates pour le navigateur, pas l'absence d'entrées-sorties, que std y compile et qui n'échoueraient qu'à l'exécution.

## Ce que devient la note de SP1b-3b

Sa section « Reporté, par héritier » donnait quatre points à SP1b-3c ; ce plan les a tous faits :

- `--firmware` (tâche 9) ;
- l'ADR du bootloader embarqué, numéro 0005 (tâche 9) ;
- les tables de partitions dans les profils (tâche 7) ;
- les retouches de forme de `boards` : la section `# Errors` de `from_named_toml` nomme `MissingFlash`, et `ProfileError::Parse` ne répète plus le remède (tâche 1 ; un refus du crate garde le sien, un refus de l'analyseur TOML reçoit le remède général, une fois).

Aucun document ne donne plus à SP1b-3c ce qu'il n'a pas fait : la tâche 1 l'a vérifié, la tâche 11 aussi. Le commentaire de `accepted_without_effect.rs` ne cite plus SP1b-3c parmi les plans à venir sans ligne.

## Décisions prises pendant l'écriture du plan

Le plan les donne dans son tableau des décisions. Les principales :

- **L'application va dans l'application d'usine, sinon dans `ota_0`.** Le plan donnait d'abord l'inverse ; le code du bootloader dit celui-ci (`bootloader_utility.c`, `bootloader_utility_get_selected_boot_partition` : « Defaulting to factory image », sinon « No factory image, trying OTA 0 »). Les deux profils n'ont pas d'application d'usine : rien ne change pour eux. Le rapport SB §F disait lui aussi l'inverse ; sa ligne est corrigée, avec une note datée.
- **Le bootloader livré est réécrit pour la taille de la flash, un bootloader donné ne l'est jamais.** Un bootloader de l'utilisateur dont l'en-tête dit plus de flash que l'appareil est refusé ; moins, c'est un avertissement.
- **Une empreinte ajoutée fausse sur un bootloader donné est un avertissement**, comme la ROM la traite sans démarrage sécurisé ; une somme de contrôle fausse reste un refus.
- **Le mode de flash n'est pas une clé du profil** : tous les profils sont en DIO, une constante de `boards`, marquée comme hypothèse (voir « Hypothèses nouvelles ») ; une clé viendra avec le premier appareil QIO ou octal.
- **La table de partitions est une suite de lignes `[[partition]]` du profil**, obligatoire, compilée et jugée au chargement (voir plus bas ce qui est refusé) ; les sous-types s'écrivent par leur nom, comme dans un CSV de `gen_esp32part.py`.
- **La révision de bloc eFuse que l'application doit accepter vient du modèle des eFuse** (`Efuse::block_revision`, v1.2), pas d'une clé du profil.
- **Les applications de référence sont prises dans les images fusionnées épinglées** : aucune construction Docker de plus, aucun fichier ni variable d'environnement de plus ; chaque test les découpe à 0x10000 et vérifie leur SHA-256 contre `SHA256SUMS.inputs` et `SHA256SUMS.inputs-nvs`. Le README de `tools/reference-firmware` donne la commande `dd` pour le faire à la main.
- **La preuve que le fichier versionné est celui de la construction de référence est un test ignoré du job `reference-boot`**, qui compare les premiers octets de `flash_hello.bin` au bootloader embarqué, plutôt qu'une étape de script.
- **Les fichiers de licence sont dans `third_party/esp-idf-bootloader/`, le binaire dans `esp32c3/`** : une licence et un avis pour tous les bootloaders ESP-IDF ; celui de l'ESP32-S3 ajoutera son fichier, une ligne de provenance et les avis Tensilica et Cadence.
- **Les refus de `flash_state.rs` nomment l'option qui a donné le fichier** : `--flash`, `--firmware` ou `--bootloader`. Le lecteur borné de `flash_file.rs` est partagé par `firmware_file.rs` au lieu d'être recopié. Ce que ce fichier a gagné en logique est dit plus bas.
- **Le refus d'une application donnée à `--flash` propose `--firmware`.**
- **`inkemu inspect` reste tel qu'il est** : décrire une image entière ou un bootloader avec son identité revient à SP2 (voir plus bas).

## Décisions prises pendant l'exécution

Les onze tâches du plan ont été suivies de corrections, chacune avec un test qui échoue avant elle et passe après, et avec ses mutations. Le plan n'est pas réécrit : une note en tête dit ce qui a changé. Ce que les corrections ont changé au comportement :

**Contrôles d'un bootloader et d'une application (`firmware`)**

- **La puce est jugée avant les segments**, comme la ROM le fait : un bootloader d'ESP32-S3 donné pour un X4 est refusé pour sa puce, les deux puces nommées, et non plus pour l'adresse de son premier segment. Le code de vitesse est jugé après la puce. Ordre complet : `.xota`, image entière, application de plus de 0x8000 octets, longueur, magie, mode, analyse de l'image, application, puce, carte mémoire, vitesse, segments, révision, taille de flash.
- **Une carte mémoire par puce** (`firmware::MEMORY_MAPS` : RAM, fenêtres de flash, zone réservée de la ROM, codes de vitesse, option de révision) remplace les constantes de l'ESP32-C3 ; les messages prennent leurs valeurs dans la carte. Un bootloader donné pour un appareil dont la puce n'a pas de carte est refusé (`UnmodelledChip`) : « the device uses ESP32-S3, and inkemu does not know where the mask ROM of ESP32-S3 loads a bootloader; choose a device whose chip inkemu models ». Aucun profil n'atteint ce cas ; un test vérifie que chaque puce de `SHIPPED` a sa carte.
- **Un fichier est nommé pour ce qu'il est** : une application de plus de 0x8000 octets donnée à `--bootloader` est nommée application (et non plus « trop longue ») ; un dump dont la table à 0x8000 ne s'analyse pas est nommé image flash entière (`FlashImageWithoutTable`) ; une image sans descripteur dont tous les segments vont en RAM, donnée à `--firmware`, est nommée bootloader. Un bootloader endommagé reçoit un remède de bootloader, et non plus celui d'un firmware.
- **Un bootloader suivi d'octets effacés est reconnu par l'empreinte de son image**, pas par celle du fichier : les 0x8000 premiers octets d'un dump du bootloader livré sont nommés « the ESP-IDF v5.5.2 bootloader inkemu ships ».
- **Le fichier entier doit tenir dans son emplacement**, pas seulement l'image qui le commence.

**Profils et tables de partitions (`boards`, `firmware::partition`)**

- **Ce que le chargement d'un profil refuse.** Depuis les tâches : un TOML mal formé, une flash absente, un identifiant qui n'est pas le nom sous lequel le profil est chargé, une puce sans bootloader livré, et une table vide ou de plus de 94 lignes, un nom de plus de 15 caractères, deux noms identiques, une partition mal alignée, qui chevauche une autre, commence avant la fin de la table ou dépasse la flash. Depuis les corrections, en plus : un nom vide ou qui contient un NUL ; deux applications d'un même sous-type (le bootloader ne garde que la dernière et réinitialise la puce sans fin, mesuré dans inkemu) ; deux partitions `otadata` ; une partition `otadata` qui ne fait pas 0x2000 octets ; une table sans application d'usine ni `ota_0`, où inkemu ne saurait pas où écrire l'application. Chaque refus dit la cause puis le remède. Les règles facultatives de `gen_esp32part.py` ligne par ligne ne sont pas portées (voir SP2).
- **La liste des profils livrés est construite par `crates/boards/build.rs`** à partir de `crates/boards/profiles/*.toml` : un appareil de plus est un fichier, sans code. Un fichier `x9.toml` ajouté à titre d'essai a été chargé par les tests et proposé par `inkemu run --device`, puis retiré.
- **Les noms, les drapeaux et l'entrée MD5 de la table du X4** sont marqués comme hypothèse, comme ceux du X3.

**Entrées-sorties de l'hôte**

- **Clippy les refuse dans `firmware`, `boards` et `soc-esp32c3`** (`disallowed-methods` et `disallowed-types` du `clippy.toml` de chaque crate : `std::fs`, `std::env`, les horloges, `std::process`, le réseau). Un appel ajouté à titre d'essai dans chacun des trois crates a fait échouer clippy. Les tests qui lisent un fichier portent une autorisation à leur seul niveau ; le script de construction de `boards` lit son répertoire, c'est son rôle. Clippy ne reconstruit pas son cache quand un `clippy.toml` change : en local, toucher un fichier source du crate ; la CI part de zéro.

**Commande (`emu-cli`)**

- **Un tube nommé donné à `--flash`, `--firmware` ou `--bootloader` est refusé avant d'être ouvert** ; le test l'établit sans borne de temps qui lui soit propre (la limite de 15 minutes des tests de la commande reste un filet), par une ouverture en écriture non bloquante qui ne réussit que si inkemu a ouvert le tube.
- **Les avertissements d'un bootloader donné et son identité** (descripteur et empreinte, ou empreinte seule) sont vérifiés à travers la commande.
- **Les remèdes nomment des options qui peuvent aller ensemble** :
  - flash de départ avec un fichier de flash qui existe : « drop --firmware, and --bootloader if it was given, to go on from the flash file, or add --reset-flash… » ;
  - fichier de flash d'une autre taille, l'image de départ venant de `--firmware` : « use it with the device it was written for, or give --flash-file a new path » ;
  - fichier trop long donné à `--bootloader` : « give the bootloader.bin of a build, or no --bootloader to use the one inkemu ships » ; à `--firmware` : « give the application .bin of a build, or give a flash image of this device to --flash instead of --firmware ».

  Le remède « même fichier » de `--firmware` et `--bootloader` ne propose jamais `--reset-flash`, et un test le tient.
- **`flash_state.rs` n'a pas gagné que du libellé** : `RunFlash::begin` recopie désormais l'option de l'image de départ dans le refus d'un fichier de flash de la mauvaise taille, une quinzaine de lignes, pour en choisir le remède ; le fichier est jugé dans `FlashState::open`, `exists` et `load`, qui ne connaissent pas l'image de départ. C'est le seul ajout de logique à ce fichier, que SP2 doit déplacer (voir plus bas).

**Avis et licences**

- **L'avis du bootloader énonce la réécriture de son en-tête** pour une flash qui ne fait pas 16 Mio (licence Apache, section 4(b)), pour que le binaire la porte ; `PROVENANCE.md` le dit aussi. Des tests de `boards` épinglent, dans l'avis du bootloader lui-même, les deux avis BSD-3-Clause (copyright, conditions et clauses de non-responsabilité de `csr.h` et d'`encoding.h`), la révision d'ESP-IDF, les sources non modifiées et la réécriture ; un autre test, l'attribution des descriptions de registres. Retirer l'un ou l'autre élément de `THIRD_PARTIES` fait échouer les tests de `boards`, et non plus seulement celui de la commande.
- **`inkemu licenses` dit ce qu'il couvre** : les éléments tiers embarqués comme données. Son aide, le README et `boards::notices` disent que les avis des bibliothèques Rust liées ne sont pas encore rassemblés.

## Ce que les tests prouvent, et ce qu'ils ne prouvent pas

Chaque comportement que le plan affirme nomme la mutation qui doit faire échouer son test ; chacune a été appliquée, et le test nommé a échoué.

- **La révision de puce d'un profil sous le minimum du bootloader livré** n'est testée qu'au niveau de la bibliothèque (`boards` et `firmware`), avec un profil v0.2 lu depuis un texte : les deux profils livrés sont v0.3, le minimum. À travers la commande, le refus de même nature est tenu par un bootloader donné qui demande v0.4.
- **Le registre ne peut pas mentir** : un test compare chaque entrée (puce, mode, révision minimale, version d'ESP-IDF, empreinte, longueur) à l'en-tête et aux octets qu'elle porte.
- **La branche « famille sans bootloader livré »** est tenue par la fonction de sélection appelée avec l'ESP32-S3 ; aucun profil ne l'atteint.
- **La réécriture de l'en-tête** n'est exercée sur une vraie image que pour 4 Mio (empreinte de la construction 4 Mio mesurée) ; les autres tailles suivent la même règle, sans construction de référence pour les comparer.
- Ce que la comparaison avec QEMU ne dit pas : les 26 lignes du bootloader sont comparées au journal QEMU, mais les lignes de l'application d'après ne sont vérifiées que par leurs jalons.
- **L'identité d'un bootloader donné, telle que l'en-tête de l'exécution l'imprime**, n'est vérifiée à travers la commande que par un test ignoré du job `rom-boot` : cet en-tête s'imprime une fois la ROM chargée. La bibliothèque la tient sans ROM.
- **Seule l'exécution prouve que le fichier versionné est celui de la construction** : un octet du bootloader changé, avec son empreinte mise à jour partout dans le code, ne fait échouer que le test ignoré du job `reference-boot`.

## Hypothèses nouvelles

`grep -rn "ASSUMPTION:" crates --include='*.rs'` rendait 86 lignes au début de la branche et en rend 87 ; dans les profils, `grep -rn "ASSUMPTION:" crates --include='*.toml'` passe de 4 à 6 lignes. Trois marqueurs s'ajoutent, aucun n'est retiré :

- `crates/boards/src/lib.rs`, au-dessus de `FLASH_MODE` : le mode SPI dans lequel la flash des appareils vendus est pilotée n'est pas établi. L'en-tête de chaque fichier Xteink connu dit DIO, mais celui d'une construction QIO le dit aussi (SB §B, ligne `qio`), et le rapport le donne pour supposé (SB §H, question ouverte 5). Ce qui le trancherait : la ligne `SPI Mode` du journal de démarrage d'un appareil vendu, ou le bootloader d'un dump. C'est cette constante qui choisit le bootloader livré à chaque profil.
- `crates/boards/profiles/x3.toml` : les noms, les drapeaux et l'entrée MD5 de la table du X3 ne sont pas établis ; les noms sont ceux du X4, aucun drapeau, l'entrée MD5 telle que `gen_esp32part.py` l'écrit (table compilée `334c9553…ae7d5e`). Les décalages et les tailles sont ceux des appareils réels. (Un dump des 0x9000 premiers octets d'un X3.)
- `crates/boards/profiles/x4.toml` : de même pour la table du X4, dont les noms, les drapeaux et l'entrée MD5 viennent des tables de CrossPoint et d'autres firmwares, pas d'un appareil vendu ; les décalages et les tailles sont ceux par lesquels les outils de flashage reconnaissent un X4. (Un dump des 0x9000 premiers octets d'un X4.)

## Reporté, par héritier

- **SP1b-3d** : inchangé (réception et interruptions UART, format de trame, côté hôte de l'USB-Serial-JTAG, pilote de console commun).
- **SP1b-3e** (CrossPoint) : aucun test CrossPoint dans ce plan. Ce que le rapport SB §C a mesuré : CrossPoint 1.6.5 démarre sur inkemu derrière le bootloader livré (l'image de FW §K.1), avec les 27 lignes du bootloader et `Set actual ota_seq=1 in otadata[0]`, puis la même panique de watchdog d'interruption que sous QEMU dans l'attente du SAR ADC (`MEPC 0x4219b962`), un redémarrage et un second démarrage ; la seconde vie interroge `SPI2.CMD` au lieu de reproduire la panique, ce que QEMU fait : écart mesuré, non expliqué. Avec `--firmware`, l'application seule suffit désormais : `inkemu run --device x4 --firmware crosspoint-1.6.5-x3-x4.bin`. Le binaire de CrossPoint n'est ni embarqué ni téléchargé par défaut (FW §N).
- **SP1b-3f**, **SP1b-4**, **SP8** : inchangés.
- **SP2** :
  - installer une nouvelle application sur un fichier de flash qui existe, en gardant NVS (une mise à jour comparable à une OTA) : c'est une action de l'interface, qui doit choisir l'emplacement et l'état d'`otadata` ; `--firmware` est une image de départ, refusée à côté d'un fichier de flash qui existe sauf avec `--reset-flash` ;
  - déplacer dans un crate de bibliothèque de l'hôte (il n'en existe pas encore ; le créer revient à SP2) la logique d'hôte restée dans `emu-cli` : `crates/emu-cli/src/flash_state.rs`, qui n'a gagné sur cette branche que du libellé et la quinzaine de lignes qui choisit le remède d'un fichier de la mauvaise taille, et trois règles que cette branche y a ajoutées (lignes à la tête de la branche) :
    - la liste des fichiers que l'exécution ne fait que lire (`--flash`, `--firmware`, `--bootloader`), comparée au fichier de flash avant toute lecture : `crates/emu-cli/src/main.rs:319-333` ;
    - la limite de lecture de l'application et du bootloader (la taille de la flash pour les deux) et le choix du fichier dont parle une `ComposeError` : `crates/emu-cli/src/firmware_file.rs:104-126` (`compose`), et le lecteur de chaque entrée à la ligne 73 (`read`) ;
    - le lecteur borné, qui refuse un fichier qui n'est pas un fichier ordinaire avant de l'ouvrir et ne lit jamais plus que la limite plus un octet : `crates/emu-cli/src/flash_file.rs:48-93` (`ReadFailure`, `read_regular_file`, `read_bounded`) ;
  - `sha256_hex` existe en double, dans `firmware::bootloader` et dans `crates/emu-cli/src/rom_store.rs` ; le crate de l'hôte en gardera un ;
  - `inkemu inspect` d'une image entière ou d'un fichier de flash (bootloader et son identité, table, application) ; aujourd'hui `inspect` refuse une image entière et décrit un bootloader comme une « ESP-IDF application image » ;
  - les messages des refus nomment les options de la commande (`--flash`, `--firmware`, `--bootloader`) : une interface les rend par leur variante typée, avec ses propres mots ;
  - un profil lu depuis un fichier de l'utilisateur (aujourd'hui seuls les profils livrés sont accessibles depuis la commande) ; avec lui, les règles facultatives que `gen_esp32part.py` applique ligne par ligne et qu'inkemu ne porte pas (une application de taille multiple de 4 Kio, une partition `ota` ou `coredump` qui n'est pas en lecture seule, une partition `nvs` en lecture-écriture d'au moins 0x3000 octets) : aucune mesure n'en a montré une qui mène à une boucle de démarrage, et les deux profils livrés les respectent.
- **SP7** (ESP32-S3) : le bootloader de l'ESP32-S3 (la même recette : 20 992 octets, `53765fcd…a023e`), son entrée de `SHIPPED`, sa carte mémoire dans `MEMORY_MAPS`, ses avis Tensilica et Cadence, et la ROM S3 dans le manifeste.
- **SP9** (distribution) : les avis des bibliothèques Rust liées dans l'exécutable, que `inkemu licenses` ne rassemble pas encore et ne prétend pas rassembler. Ils sont sous MIT (`goblin`, `nix`, `scroll`, `bytes`, `strsim`…), BSD-3-Clause (`subtle`), ISC (`rustls-webpki`, `untrusted`), Apache-2.0 et ISC (`ring`), Zlib (`zlib-rs`), CDLA-Permissive-2.0 (`webpki-roots`), entre autres ; `cargo metadata` en donne la liste. Rien n'est distribué avant SP9. Windows, inchangé.
- **Quand un profil le demandera** : un bootloader « silencieux » (niveau ERROR, retour arrière, validation sautée au réveil), seconde entrée du registre qu'un profil ou une interface choisirait ; une construction QIO ou octale, avec une clé du profil qui nomme le mode de flash.

## Lignes de la table des registres acceptés, par héritier

Inchangées : SP1b-3d 8, SP1b-3f 10, SP1b-4 28, SP2 26, SP5a 8, SP5b 2, SP8 22. Ce plan n'ajoute ni ne retire aucun registre.

## Tests et coût en CI

`cargo test --workspace` : 1 093 tests passent, 65 sont ignorés (18 de `real_rom`, 13 de `reference_boot`, 33 de `run_cli`, 1 de `inspect_cli`) ; la note de SP1b-3b en comptait 1 005 et 60. La CI exécute les tests ignorés avec leur compte exact :

- job `rom-boot` : `real_rom` 18 (inchangé), les tests de la commande sans firmware de référence passent de 24 à 25 (le test de plus vérifie l'identité d'un bootloader donné dans l'en-tête de l'exécution) ;
- job `reference-boot` : `reference_boot` 13 (inchangé), les tests de la commande sur le firmware de référence passent de 4 à 8.

Le job `check` construit aussi `firmware` et `boards` pour WebAssembly, et son étape clippy y refuse les entrées-sorties de l'hôte. Le cache du firmware de référence est recalculé une fois, parce que le README et les commentaires de `build.sh` de `tools/reference-firmware` changent : la construction Docker a lieu une fois dans la CI, pas sur la machine de développement.

Durées des tests ignorés, en version optimisée, sur la machine de développement (16 cœurs, charge moyenne de 7 à 12), telles que le lanceur de tests les affiche : `real_rom` 1,5 s, les 25 tests de la commande 5,7 s, `reference_boot` 6,5 s, les 8 tests de la commande sur le firmware de référence 12,7 s.

Aucune dépendance n'est ajoutée : `boards` dépend désormais de `firmware`, un crate de l'espace de travail ; `cargo deny check` passe.

## À reporter dans la spec

- **§5.1, entrées acceptées** : « la table de partitions choisie dans le profil (stock X4, stock X3, CrossPoint) » devient « la table du profil » : chaque profil en énonce une, et celle de CrossPoint est celle du X4, octet pour octet. L'application va dans l'application d'usine, sinon dans `ota_0` : ce que le bootloader démarre quand `otadata` est vierge. `otadata` reste vierge.
- **§5.1, refus** : s'ajoutent ceux de §10.3 critère 7 que `--firmware` tient : une application plus grande que son emplacement, une image entière donnée comme application, un bootloader donné comme application et l'inverse, une révision de puce ou de bloc eFuse hors des bornes de l'application, un bootloader qui demande une puce plus récente. `--firmware` est, comme `--flash`, une image de départ : refusée à côté d'un fichier de flash qui existe, sauf avec `--reset-flash`.
- **§3.3, profils** : « les tables de partitions proposées » devient une table par profil, en lignes `[[partition]]`, validée au chargement : contre la taille de la flash, et contre ce que le bootloader démarre (un seul emplacement par sous-type d'application, une seule partition `otadata` de 0x2000 octets, une application d'usine ou un `ota_0`). « Ajouter un appareil revient à écrire un profil » est désormais exact à la lettre pour une puce connue : un fichier `profiles/<id>.toml`, sans code.
- **§3.4** : la branche y ajoute une phrase qui renvoie à l'ADR 0005, qui la précise : un bootloader par famille de puce, et par famille de mode de flash quand un profil en demande une autre, la taille de flash écrite dans l'en-tête. La ligne du tableau peut devenir « embarqué, un par famille de puce et de mode de flash ».
- **§6.4, CLI** : `inkemu licenses` imprime les avis des éléments tiers qu'inkemu embarque comme données (ceux des bibliothèques Rust liées viennent avec SP9) ; `--bootloader` remplace le bootloader livré.
