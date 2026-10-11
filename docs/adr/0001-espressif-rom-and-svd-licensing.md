# ADR 0001 — Licences de la ROM et des fichiers SVD Espressif

- Date : 2026-10-11
- Statut : accepté

## Contexte

La ROM masquée de l'ESP32-C3/S3 est indispensable : le bootloader et l'application appellent ses fonctions, et l'émulateur doit donc en charger une image.
inkemu est publié sous Apache-2.0. Il faut décider si les images de ROM et les fichiers SVD (descriptions de registres CMSIS-SVD) peuvent être embarqués dans le dépôt et dans les binaires distribués, ou s'ils doivent être récupérés à l'exécution ou à la construction.
La première tâche de SP1b dépend de cette décision.

Tous les faits ci-dessous ont été relevés le 2026-10-11 sur les dépôts et releases publics d'Espressif. Aucun binaire de ROM n'a été téléchargé pour établir cet ADR : les empreintes proviennent des métadonnées de release et des fichiers de somme de contrôle publiés. Les deux fichiers SVD, qui sont du XML texte, ont en revanche été téléchargés pour calculer leur SHA-256, car Espressif n'en publie pas pour ces releases.

## Faits constatés

| Élément | Dépôt / release | Licence SPDX | Fichiers ROM binaires présents ? | Source (URL) |
|---|---|---|---|---|
| ROM, ELF | `espressif/esp-rom-elfs`, release `20260528` (publiée le 2026-05-28, la plus récente) | Apache-2.0 (dépôt et texte de licence). Le README précise que la ROM « n'est pas open-source » mais que les fichiers ELF sont cédés sous Apache-2.0, au titre de la forme « Object » de la licence | **ELF uniquement**, dans l'archive `esp-rom-elfs-20260528.tar.gz`. Les notes de release listent ESP32-C3 `REV0`/`REV101`/`REV3` et ESP32-S3 `REV0`. Les assets de la release sont l'archive et son fichier de somme de contrôle : aucun `.bin` brut | https://github.com/espressif/esp-rom-elfs/blob/master/LICENSE ; https://github.com/espressif/esp-rom-elfs/blob/master/README.md ; https://github.com/espressif/esp-rom-elfs/releases/tag/20260528 |
| ROM, `.bin` brut | `espressif/qemu`, branche `esp-develop` : `pc-bios/esp32c3-rom.bin` (393 216 octets) et `pc-bios/esp32s3_rev0_rom.bin` (393 216 octets) | Dépôt : `NOASSERTION` (« Other »). Le `LICENSE` de QEMU dit seulement que les fichiers de firmware sont des programmes séparés, « avec des licences séparées ». **Aucune licence n'est énoncée pour ces deux `.bin`** : `pc-bios/README` ne les mentionne pas et le dépôt n'a pas de fichier de licence dédié | Oui, dans l'arbre du dépôt. Présence dans les tarballs de release `qemu-riscv32-softmmu-*` et `qemu-xtensa-softmmu-*` : **inconnu** (archives non téléchargées) | https://github.com/espressif/qemu/blob/esp-develop/LICENSE ; https://github.com/espressif/qemu/blob/esp-develop/pc-bios/README ; https://github.com/espressif/qemu/blob/esp-develop/pc-bios/esp32c3-rom.bin ; https://github.com/espressif/qemu/blob/esp-develop/pc-bios/esp32s3_rev0_rom.bin ; https://github.com/espressif/qemu/releases/tag/esp-develop-9.2.2-20260417 |
| Code tiers dans la ROM | Documentation ESP-IDF, section « ROM Source Code Copyrights » | Newlib (BSD), Xtensa libhal (MIT), TinyBasic Plus (MIT), miniz (domaine public), TJpgDec (licence ChaN : conserver la mention de copyright dans les redistributions de code source), morceaux de la pile USB de Zephyr (Apache-2.0 et BSD-3-Clause) | Sans objet | https://docs.espressif.com/projects/esp-idf/en/latest/esp32c3/COPYRIGHT.html |
| SVD | `espressif/svd`, release `2024-05-09` (la plus récente ; dernier commit sur `main` : `be20aa12560889d6125d144cdb48cf615ac17628`, 2024-05-09) | Apache-2.0. Le README énonce que les fichiers du dépôt sont distribués sous Apache-2.0 | Sans objet. Ce sont des fichiers XML : `esp32c3.svd` (1 366 305 octets), `esp32s3.svd` (2 781 196 octets). Le README prévient qu'ils sont « en cours de réalisation » et peuvent omettre des périphériques, interruptions ou registres | https://github.com/espressif/svd/blob/main/LICENSE ; https://github.com/espressif/svd/blob/main/README.md ; https://github.com/espressif/svd/releases/tag/2024-05-09 |

Empreintes publiées ou mesurées :

| Fichier | SHA-256 | Provenance de l'empreinte |
|---|---|---|
| `esp-rom-elfs-20260528.tar.gz` (4 902 355 octets) | `caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46` | **Publiée** par Espressif : fichier `esp-rom-elfs-20260528-checksum.sha256` et champ `digest` de l'asset GitHub, identiques. https://github.com/espressif/esp-rom-elfs/releases/download/20260528/esp-rom-elfs-20260528-checksum.sha256 |
| ELF ROM ESP32-C3 (par révision) | non publiée — à calculer au premier téléchargement et à épingler alors | Aucune empreinte par fichier n'est publiée, seulement celle de l'archive |
| ELF ROM ESP32-S3 `REV0` | non publiée — à calculer au premier téléchargement et à épingler alors | Idem |
| `esp32c3.svd`, release `2024-05-09` | `9ef33dbff56d807996fd8edfa4aa4ad008b5a861c241d36280caa0dedbbca41a` | **Mesurée** le 2026-10-11 sur https://github.com/espressif/svd/releases/download/2024-05-09/esp32c3.svd. Le champ `digest` de l'asset est `null`, et Espressif ne publie pas de somme pour les SVD |
| `esp32s3.svd`, release `2024-05-09` | `d7629249eba2ad660a960530d124966e9274c8228341380dd4102f251e879e17` | **Mesurée** le 2026-10-11 sur https://github.com/espressif/svd/releases/download/2024-05-09/esp32s3.svd |

À titre d'information uniquement, l'API de contenu de GitHub donne les identifiants d'objet git (SHA-1, pas SHA-256) des `.bin` de QEMU sur `esp-develop` : `3f59017cbe39baf6ca4ce25ff9ba6c94a68d25b6` pour `esp32c3-rom.bin` et `e93eb01abbb7f9c44be38d55ef0efaedbd10caf4` pour `esp32s3_rev0_rom.bin`. Ils ne servent pas de référence d'intégrité dans inkemu, puisque ces fichiers ne sont pas retenus comme source.

Points établis comme **inconnus** à la date de cet ADR :

- le SHA-256 de chaque ELF de ROM pris individuellement ;
- les noms exacts des fichiers dans l'archive : le README indique le motif `<CHIP_NAME>_rev<REV>_rom.elf` ;
- le fait que ces ELF contiennent le contenu complet de la ROM (segments chargeables avec les données), et pas seulement les symboles et le code : ils sont publiés pour le débogage, et l'archive n'a pas été ouverte ;
- le contenu des tarballs binaires de `espressif/qemu`, et le régime de licence de leurs fichiers de ROM ;
- la révision de l'ESP32-C3 réellement présente sur les appareils visés.

## Décision

**ROM : `download-on-first-run`.**

Justification :

1. La seule source dont la licence est explicite est `esp-rom-elfs`, qui publie des fichiers ELF dans une archive multi-puces. Les images plates `.bin` que fournit QEMU n'ont aucune licence énoncée : en l'absence de licence explicite, la redistribution n'est pas fondée et seul le téléchargement est admissible pour ces fichiers.
2. L'ELF doit encore être converti en image plate par inkemu, et on ignore à ce jour si ses segments contiennent la ROM complète. S'engager à embarquer le résultat d'une transformation non validée serait prématuré.
3. Embarquer la ROM ferait porter à chaque distribution d'inkemu les mentions de copyright du code tiers qu'elle contient (liste ci-dessus), et figerait une révision de ROM dans les binaires.
4. Le téléchargement d'une archive épinglée dont le SHA-256 est publié par Espressif offre une vérification d'intégrité sans introduire de binaire dans le dépôt.
5. Le choix est réversible : la licence Apache-2.0 d'`esp-rom-elfs` permet de revenir sur `embedded` plus tard (voir « Conséquences »).

Téléchargement exact :

- URL : `https://github.com/espressif/esp-rom-elfs/releases/download/20260528/esp-rom-elfs-20260528.tar.gz`
- SHA-256 de l'archive : `caa463d3cbef2430a5a35847c1d9f2f152403b17a802050927ff60c8da54fe46` (4 902 355 octets).
- SHA-256 des fichiers ROM ESP32-C3 et ESP32-S3 : **non publiés par Espressif.** Ils doivent être calculés lors du premier téléchargement de l'archive épinglée, puis inscrits dans le manifeste de ROM du dépôt (voir « Conséquences »). Ils ne sont volontairement pas inventés ici.

**SVD : `vendored`.**

Justification : la licence Apache-2.0 est explicite et s'applique aux fichiers du dépôt `espressif/svd`. Elle est identique à celle d'inkemu. Il s'agit de texte XML (environ 4,1 Mo pour les deux fichiers), que la construction peut ainsi utiliser sans réseau ; un téléchargement à la construction rendrait celle-ci dépendante de la disponibilité de GitHub pour un gain de taille marginal.

Conditions : copier les deux fichiers à l'identique depuis la release `2024-05-09`, avec le texte de la licence Apache-2.0 d'`espressif/svd` et un fichier de provenance (dépôt, tag, URL, SHA-256 ci-dessus). Conserver les mentions de copyright présentes dans les fichiers, et signaler toute modification éventuelle.

## Conséquences

- **Cache local de la ROM** : `<data_dir>/inkemu/rom/`, où `<data_dir>` est le répertoire de données utilisateur de la plateforme. Le premier lancement qui a besoin d'une ROM absente télécharge l'archive épinglée, **vérifie son SHA-256 avant toute extraction**, extrait les seuls ELF nécessaires et les convertit en images. Les écritures se font vers un fichier temporaire puis renommage, pour qu'un téléchargement interrompu ne laisse jamais un fichier partiel à l'emplacement final.
- **Manifeste de ROM versionné dans le dépôt** : tag de release, URL, SHA-256 de l'archive, et, une fois calculés, SHA-256 de chaque ELF (C3 par révision, S3 `REV0`). La première tâche de SP1b doit ouvrir l'archive, relever les noms de fichiers, vérifier que les segments chargeables contiennent bien la ROM complète, et inscrire ces empreintes. Si la vérification échoue, cet ADR doit être révisé avant d'écrire le chargeur.
- **Vérification à chaque lancement** : les empreintes des images en cache sont revérifiées contre le manifeste.
- **Messages d'erreur** : ROM absente et réseau indisponible, ROM altérée ou empreinte différente, archive téléchargée dont l'empreinte ne correspond pas. Chacun doit nommer le fichier, l'empreinte attendue et celle observée, et indiquer comment fournir la ROM manuellement. Une ROM altérée n'est jamais chargée.
- **ROM fournie par l'utilisateur** : l'émulateur devra accepter un répertoire de ROM explicite, soumis à la même vérification d'empreinte, pour les environnements hors ligne.
- **Les `.bin` de QEMU ne sont ni embarqués ni téléchargés par défaut**, tant qu'une licence explicite ne les couvre pas.
- **Dépôt** : aucun binaire de ROM n'est commité. Le dépôt contient en revanche `third_party/espressif-svd/` (deux SVD, licence, provenance).
- **Avis dans la documentation** : indiquer que la ROM est le code d'Espressif, qu'elle n'est pas open-source, et qu'elle est utilisée sous Apache-2.0 selon le README d'`esp-rom-elfs`.
- **Révision de l'ADR** : à rouvrir si Espressif publie des empreintes par fichier ou des images plates sous licence explicite, ou si un besoin de distribution hors ligne impose d'embarquer la ROM.
