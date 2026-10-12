# SP1b-3c — Note de passation

Plan : `docs/superpowers/plans/2026-10-13-sp1b-3c-firmware.md` (`--firmware` : une image d'application démarre derrière le bootloader qu'inkemu embarque). Décision durable : ADR 0005 (le bootloader de second étage, embarqué, choisi par le profil, remplaçable).

Les numéros de décision cités ici (C1 à C30) se résolvent dans le plan versionné, au tableau « Design decisions taken for this plan », pas dans l'ADR ni dans le code : C1 à C15 ont été donnés au plan, C16 à C30 pris pendant son écriture.

## Résultat

- **`inkemu run --device x4 --firmware hello_world.bin` démarre l'application seule**, derrière le bootloader ESP-IDF v5.5.2 qu'inkemu embarque et la table de partitions d'origine du X4 : en mode strict, toute la vie de l'application, son redémarrage et le second démarrage, sans accès non modélisé. Au premier démarrage, le bootloader imprime ses 26 lignes, celles de la capture QEMU de la même flash (`golden_hello_ota_boot.log`) aux deux différences connues près (révision de bloc eFuse v1.2, mode SPI DIO), dont la table du X4, `No factory image, trying OTA 0` et `Set actual ota_seq=1 in otadata[0]` ; au second, ni l'une ni l'autre de ces deux lignes.
- **La flash composée est celle de la construction de référence** : pour `hello_world`, `9ddacb60…11bc89`, l'image `flash_hello_ota.bin` du rapport TO §2 ; pour `nvs_rw_value_cxx`, `87832c6d…f28fec`, l'image épinglée `flash_nvs_ota.bin`. Le SHA-256 est imprimé (`flash at start`), et deux exécutions ont la même empreinte de trace : `c6b24a1cc226ed4a` pour `--firmware hello_world.bin --strict-mmio --max-cycles 40000001`, 11 403 054 µs d'appareil, 26 637 404 instructions, épinglée par un test. L'empreinte de la vie de l'image fusionnée, `10174cc2726c3d77`, ne change pas.
- **Le compteur de NVS continue d'une exécution à l'autre à partir de l'application seule** : une première exécution `--firmware nvs_rw_value_cxx.bin --flash-file x4.bin` écrit `otadata` et affiche `The value is not initialized yet!` ; une seconde, avec le fichier seul, démarre à froid et affiche `Restart counter = 1`.
- **Chaque refus est nommé avant le démarrage**, avec la cause puis le remède, et testé à travers la commande : un fichier qui n'est pas une image, un `.xota` chiffré, une image ESP32-S3 pour un X4, une application plus grande que son emplacement, une empreinte fausse, une image entière (« `--flash` »), un bootloader donné comme firmware (« `--bootloader` »), une application donnée comme bootloader (« `--firmware` »), un bootloader qui demande une puce plus récente (il la réinitialiserait sans fin).
- **`inkemu licenses`** imprime le `NOTICE` d'inkemu puis l'avis et la licence de chaque partie tierce du binaire ; le même texte vient de `boards::notices`, pour la page « À propos » d'une interface.
- **Tout passe par des bibliothèques** : composition, registre des bootloaders, contrôles et tables de partitions sont dans `firmware` et `boards`, avec des erreurs typées ; les deux crates se compilent pour WebAssembly, et la CI le vérifie. `emu-cli` lit les fichiers, appelle `DeviceProfile::compose_flash` et imprime.

## Ce que devient la note de SP1b-3b

Sa section « Reporté, par héritier » donnait quatre points à SP1b-3c ; ce plan les a tous faits :

- `--firmware` (tâche 9) ;
- l'ADR du bootloader embarqué, numéro 0005 (tâche 9) ;
- les tables de partitions dans les profils (tâche 7) ;
- les retouches de forme de `boards` : la section `# Errors` de `from_named_toml` nomme `MissingFlash`, et `ProfileError::Parse` ne répète plus le remède (tâche 1 ; un refus du crate garde le sien, un refus de l'analyseur TOML reçoit le remède général, une fois).

Aucun document ne donne plus à SP1b-3c ce qu'il n'a pas fait : la tâche 1 l'a vérifié, la tâche 11 aussi. Le commentaire de `accepted_without_effect.rs` ne cite plus SP1b-3c parmi les plans à venir sans ligne.

## Décisions prises pendant l'écriture du plan

Elles sont dans le tableau des décisions du plan, de C16 à C30. Les principales :

- **L'application va dans l'application d'usine, sinon dans `ota_0`** (C16, qui corrige C7). C7, tel qu'il a été donné au plan, disait l'inverse ; le code du bootloader dit celui-ci (`bootloader_utility.c`, `bootloader_utility_get_selected_boot_partition` : « Defaulting to factory image », sinon « No factory image, trying OTA 0 »). Les deux profils n'ont pas d'application d'usine : rien ne change pour eux.
- **Le bootloader livré est réécrit pour la taille de la flash, un bootloader donné ne l'est jamais** (C17). Un bootloader de l'utilisateur dont l'en-tête dit plus de flash que l'appareil est refusé ; moins, c'est un avertissement.
- **Une empreinte ajoutée fausse sur un bootloader donné est un avertissement** (C18), comme la ROM la traite sans démarrage sécurisé ; une somme de contrôle fausse reste un refus.
- **Le mode de flash n'est pas une clé du profil** (C19) : tous les profils sont en DIO, une constante de `boards` ; une clé viendra avec le premier appareil QIO ou octal.
- **La table de partitions est une suite de lignes `[[partition]]` du profil** (C20), obligatoire, compilée et jugée contre la taille de la flash au chargement ; les sous-types s'écrivent par leur nom, comme dans un CSV de `gen_esp32part.py`.
- **La révision de bloc eFuse que l'application doit accepter vient du modèle des eFuse** (C21, `Efuse::block_revision`, v1.2), pas d'une clé du profil.
- **Les applications de référence sont prises dans les images fusionnées épinglées** (C25) : aucune construction Docker de plus, aucun fichier ni variable d'environnement de plus ; chaque test les découpe à 0x10000 et vérifie leur SHA-256 contre `SHA256SUMS.inputs` et `SHA256SUMS.inputs-nvs`. Le README de `tools/reference-firmware` donne la commande `dd` pour le faire à la main.
- **La preuve de C1 « le fichier versionné est celui de la construction de référence » est un test ignoré du job `reference-boot`** (C26), qui compare les premiers octets de `flash_hello.bin` au bootloader embarqué, plutôt qu'une étape de script.
- **Les fichiers de licence sont dans `third_party/esp-idf-bootloader/`, le binaire dans `esp32c3/`** (C27) : une licence et un avis pour tous les bootloaders ESP-IDF ; celui de l'ESP32-S3 ajoutera son fichier, une ligne de provenance et les avis Tensilica et Cadence.
- **Les refus de `flash_state.rs` nomment l'option qui a donné le fichier** (C28) : `--flash`, `--firmware` ou `--bootloader`. C'est un changement de libellé, pas de logique : la dette que SP2 hérite n'a pas grandi. Le lecteur borné de `flash_file.rs` est partagé par `firmware_file.rs` au lieu d'être recopié.
- **Le refus d'une application donnée à `--flash` propose `--firmware`** (C29).
- **`inkemu inspect` reste tel qu'il est** (C30) : décrire une image entière ou un bootloader avec son identité revient à SP2 (voir plus bas).

## Ce que les tests prouvent, et ce qu'ils ne prouvent pas

Chaque comportement que le plan affirme nomme la mutation qui doit faire échouer son test ; chacune a été appliquée, et le test nommé a échoué.

- **La révision de puce d'un profil sous le minimum du bootloader livré** n'est testée qu'au niveau de la bibliothèque (`boards` et `firmware`), avec un profil v0.2 lu depuis un texte : les deux profils livrés sont v0.3, le minimum. À travers la commande, le refus de même nature est tenu par un bootloader donné qui demande v0.4.
- **Le registre ne peut pas mentir** : un test compare chaque entrée (puce, mode, révision minimale, version d'ESP-IDF, empreinte, longueur) à l'en-tête et aux octets qu'elle porte.
- **La branche « famille sans bootloader livré »** est tenue par la fonction de sélection appelée avec l'ESP32-S3 ; aucun profil ne l'atteint.
- **La réécriture de l'en-tête** n'est exercée sur une vraie image que pour 4 Mio (empreinte de la construction 4 Mio mesurée) ; les autres tailles suivent la même règle, sans construction de référence pour les comparer.
- Ce que la comparaison avec QEMU ne dit pas : les 26 lignes du bootloader sont comparées au journal QEMU, mais les lignes de l'application d'après ne sont vérifiées que par leurs jalons.

## Hypothèses nouvelles

`grep -rn "ASSUMPTION:" crates --include='*.rs'` rend toujours 86 lignes. Un marqueur s'ajoute dans un profil :

- `crates/boards/profiles/x3.toml` : les noms, les drapeaux et l'entrée MD5 de la table du X3 ne sont pas établis ; les noms sont ceux du X4, aucun drapeau, l'entrée MD5 telle que `gen_esp32part.py` l'écrit (table compilée `334c9553…ae7d5e`). Les décalages et les tailles sont ceux des appareils réels. (Un dump des 0x9000 premiers octets d'un X3.)

## Reporté, par héritier

- **SP1b-3d** : inchangé (réception et interruptions UART, format de trame, côté hôte de l'USB-Serial-JTAG, pilote de console commun).
- **SP1b-3e** (CrossPoint) : aucun test CrossPoint dans ce plan. Ce que le rapport SB §C a mesuré : CrossPoint 1.6.5 démarre sur inkemu derrière le bootloader livré (l'image de FW §K.1), avec les 27 lignes du bootloader et `Set actual ota_seq=1 in otadata[0]`, puis la même panique de watchdog d'interruption que sous QEMU dans l'attente du SAR ADC (`MEPC 0x4219b962`), un redémarrage et un second démarrage ; la seconde vie interroge `SPI2.CMD` au lieu de reproduire la panique, ce que QEMU fait : écart mesuré, non expliqué. Avec `--firmware`, l'application seule suffit désormais : `inkemu run --device x4 --firmware crosspoint-1.6.5-x3-x4.bin`. Le binaire de CrossPoint n'est ni embarqué ni téléchargé par défaut (FW §N).
- **SP1b-3f**, **SP1b-4**, **SP8** : inchangés.
- **SP2** :
  - installer une nouvelle application sur un fichier de flash qui existe, en gardant NVS (une mise à jour comparable à une OTA) : c'est une action de l'interface, qui doit choisir l'emplacement et l'état d'`otadata` ; `--firmware` est une image de départ, refusée à côté d'un fichier de flash qui existe sauf avec `--reset-flash` ;
  - déplacer `crates/emu-cli/src/flash_state.rs` dans un crate de bibliothèque (la dette que ce plan n'a pas fait grandir) ;
  - `inkemu inspect` d'une image entière ou d'un fichier de flash (bootloader et son identité, table, application) ; aujourd'hui `inspect` refuse une image entière et décrit un bootloader comme une « ESP-IDF application image » ;
  - les messages des refus nomment les options de la commande (`--flash`, `--firmware`, `--bootloader`) : une interface les rend par leur variante typée, avec ses propres mots ;
  - un profil lu depuis un fichier de l'utilisateur (aujourd'hui seuls les profils livrés sont accessibles depuis la commande).
- **SP7** (ESP32-S3) : le bootloader de l'ESP32-S3 (la même recette : 20 992 octets, `53765fcd…a023e`), son entrée du registre, ses avis Tensilica et Cadence, et la ROM S3 dans le manifeste.
- **SP9** (distribution) : les licences des bibliothèques Rust liées dans l'exécutable, que `inkemu licenses` ne liste pas encore ; Windows, inchangé.
- **Quand un profil le demandera** : un bootloader « silencieux » (niveau ERROR, retour arrière, validation sautée au réveil), seconde entrée du registre qu'un profil ou une interface choisirait ; une construction QIO ou octale, avec une clé du profil qui nomme le mode de flash.

## Lignes de la table des registres acceptés, par héritier

Inchangées : SP1b-3d 8, SP1b-3f 10, SP1b-4 28, SP2 26, SP5a 8, SP5b 2, SP8 22. Ce plan n'ajoute ni ne retire aucun registre.

## Tests et coût en CI

`cargo test --workspace` : 1 064 tests passent, 64 sont ignorés (18 de `real_rom`, 13 de `reference_boot`, 32 de `run_cli`, 1 de `inspect_cli`). La CI exécute les tests ignorés avec leur compte exact :

- job `rom-boot` : `real_rom` 18, les tests de la commande sans firmware de référence 24 (inchangés) ;
- job `reference-boot` : `reference_boot` 13 (inchangé), les tests de la commande sur le firmware de référence passent de 4 à 8.

Le job `check` construit aussi `firmware` et `boards` pour WebAssembly. Le cache du firmware de référence est recalculé une fois, parce que le README de `tools/reference-firmware` change : la construction Docker a lieu une fois dans la CI, pas sur la machine de développement.

Durées des tests ignorés, en version optimisée, sur la machine de développement (16 cœurs, charge moyenne de 2 à 4), telles que le lanceur de tests les affiche : `real_rom` 4,5 s, les 24 tests de la commande 5,9 s, `reference_boot` 21,3 s, les 8 tests de la commande sur le firmware de référence 11,3 s.

Aucune dépendance n'est ajoutée : `boards` dépend désormais de `firmware`, un crate de l'espace de travail ; `cargo deny check` passe.

## À reporter dans la spec

- **§5.1, entrées acceptées** : « la table de partitions choisie dans le profil (stock X4, stock X3, CrossPoint) » devient « la table du profil » : chaque profil en énonce une, et celle de CrossPoint est celle du X4, octet pour octet. L'application va dans l'application d'usine, sinon dans `ota_0` : ce que le bootloader démarre quand `otadata` est vierge. `otadata` reste vierge.
- **§5.1, refus** : s'ajoutent ceux de §10.3 critère 7 que `--firmware` tient : une application plus grande que son emplacement, une image entière donnée comme application, un bootloader donné comme application et l'inverse, une révision de puce ou de bloc eFuse hors des bornes de l'application, un bootloader qui demande une puce plus récente. `--firmware` est, comme `--flash`, une image de départ : refusée à côté d'un fichier de flash qui existe, sauf avec `--reset-flash`.
- **§3.3, profils** : « les tables de partitions proposées » devient une table par profil, en lignes `[[partition]]`, validée contre la taille de la flash au chargement.
- **§3.4** : inchangée ; l'ADR 0005 en est la mise en œuvre (un bootloader par famille de puce et de mode de flash, la taille de flash écrite dans l'en-tête).
- **§6.4, CLI** : `inkemu licenses` imprime les avis des parties tierces du binaire ; `--bootloader` remplace le bootloader livré.
