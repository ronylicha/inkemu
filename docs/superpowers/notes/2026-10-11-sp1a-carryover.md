# SP1a — points reportés au sous-projet 1b

Bilan de l'exécution du plan `docs/superpowers/plans/2026-10-11-sp1a-core-rv32-firmware.md` : les 12 tâches sont livrées, chacune relue, puis la branche a été relue en entier. Ce document regroupe ce qui a été **volontairement reporté**. Le plan SP1b doit reprendre chaque point ou l'écarter explicitement.

## Contrats sur lesquels SP1b s'appuie

- ~~**`Bus::page_version`** : chaque implémentation (flash, ROM, MMIO) doit le tenir honnêtement, sinon le cache de décodage ne s'invalide jamais. Les versions sont tenues par page physique absolue de 4 Kio. Sur le C3, la SRAM est vue à la fois en IRAM et en DRAM : une écriture par l'alias DRAM doit invalider le code décodé à l'adresse IRAM.~~ **Levé par SP1b-1** : le bus du SoC (`crates/soc-esp32c3/src/bus.rs`) tient `page_version` pour chaque région, et la SRAM vue par l'alias IRAM et par l'alias DRAM partage les mêmes versions (test du bus). Le contrat reste valable pour les futures implémentations de `Bus` (flash, SP1b-2).
- **`Bus::read`** : une lecture sur 8 ou 16 bits renvoie une valeur alignée à droite et étendue par des zéros. Le cœur masque par sécurité, mais les périphériques doivent respecter ce contrat.
- **`Bus::fetch_stall`** : c'est le crochet par lequel le modèle du cache flash facture un fetch servi par le cache de décodage.
- **`DecodeCache`** : il faut un `clear()` appelé au reset et à la restauration d'un instantané, et un `Debug` écrit à la main (le `Debug` dérivé affiche la `HashMap` dans un ordre non déterministe). Le cache n'évince jamais de page, et chaque changement de version remet à zéro environ 40 Ko de slots : à mesurer.

## Comportements du cœur à arbitrer (lockstep QEMU)

- ~~La politique d'alignement réelle du C3 (`MisalignedPolicy`) doit rester cohérente à trois endroits : le YAML ISA, la configuration JSON de Sail et l'option `--allow-misaligned` (voir `tools/conformance/README.md`).~~ **Levé par SP1b-1 pour le SoC** : la machine ESP32-C3 exécute les accès non alignés au lieu de les piéger (`MisalignedPolicy::Allow`). Preuve : la ROM rev0 fait un `lw` non aligné dans `_init` (`docs/research/esp32c3-boot-chain.md`, section M). Il reste que la cohérence à trois endroits ne concerne plus que le banc de conformité : `inkemu rvtest` piège par défaut, car il teste un cœur générique.
- Une erreur `Misaligned` remontée par le bus est aujourd'hui traduite en faute d'accès (cause 5/7) et non en désalignement (cause 4/6) : à trancher.
- ~~Une écriture dans `mcycle`/`mcycleh` doit l'emporter sur l'incrément de l'instruction qui l'écrit, comme c'est déjà le cas pour `minstret`.~~ **Levé par SP1b-1.**
- L'entrée en interruption ne coûte aucun cycle, et `StepOutcome::Interrupted` n'a pas de champ `cycles`.
- ~~`set_interrupt_pending` masque silencieusement la ligne à 0..31 : ajouter un `debug_assert`.~~ **Levé par SP1b-1.**
- Les CSR PMP et les déclencheurs de débogage du C3 sont absents. Les tests `rv32mi-p-pmpaddr` et `rv32mi-p-breakpoint` sont exclus, et le groupe arch-test `pmp32` est désactivé. Le test CLI `failing_test_exits_one…` repose sur l'échec de `pmpaddr` : il cassera quand la PMP arrivera et devra utiliser un ELF synthétique.

## Firmware et chargeur

- La détection d'un dump flash complet repose sur l'heuristique « `AA 50` à `0x8000` » et peut refuser une vraie application par coïncidence (de l'ordre de 1e-4 par image). Le chargeur SP1b, qui gère les dumps, doit la remplacer par `PartitionTable::parse` (magics et MD5). Les dumps de `0x8C00` à `0x8FFF` octets ne sont pas détectés.
- Le message de `BadFlashSize` induit en erreur pour une image valide dont le code de taille de flash dépasse la table supportée.
- `app_desc.rs` : il manque des tests pour une chaîne pleine largeur non terminée, de l'UTF-8 invalide, un segment court portant le magic et un mauvais magic. Les octets après le hash SHA-256 sont ignorés, ce qui n'est ni documenté ni testé.
- `partition.rs` : les règles d'`otadata` (nombre, taille) et les noms contenant un NUL ne sont pas gérés, plusieurs constantes ne sont pas nommées, et `NameTooLong` couvre aussi les noms non ASCII.

## CLI

- `inspect` :
  - `SpiMode` est affiché via `{:?}` : il faut un `Display` (« DIO »).
  - ~~Une écriture vers un stdout fermé panique (EPIPE).~~ **Levé par SP1b-1.**
  - Le fichier est lu sans limite de taille.
  - Le nettoyage des chaînes laisse passer les caractères bidirectionnels et séparateurs (usurpation visuelle uniquement).
  - Le libellé de région ne regarde que l'adresse de début du segment.
- `rvtest` :
  - Les chemins affichés dans les erreurs ne sont pas nettoyés.
  - La détection de `tohost` ignore la taille de l'accès.
  - Le chargeur ELF utilise `p_paddr`.
  - L'empreinte de trace lit le mot d'instruction après l'exécution.

## Outillage et CI

- La couverture n'est contrôlée que globalement (80 %). Ni le minimum par crate ni les 100 % exigés sur le décodeur et sur le parsing d'images ne sont vérifiés.
- Épinglage :
  - `riscv-arch-test` est épinglé par tag, pas par SHA.
  - L'archive Sail n'a pas de somme de contrôle.
  - Ubuntu, apt et les dépendances Python ne sont pas épinglés.
  - Les actions GitHub ne sont pas épinglées par SHA.
  - `cargo` ne tourne pas avec `--locked` en CI.
- Le job `arch-test` n'a pas de cache Docker. Il est déclenché par des chemins qui n'incluent pas `rust-toolchain.toml`, `deny.toml` ni `clippy.toml`.
- ~~La CI n'a encore jamais tourné sur GitHub.~~ **Levé le 2026-10-11** : le premier run sur `main` (commit `95ef0c6`) est vert sur ses trois jobs. Le journal confirme que les 92 tests d'architecture ont réellement tourné (39 I, 8 M, 29 C, 1 Zifencei, 15 privilege, 0 échec) et que la couverture atteint 98,77 % des lignes.

## Vérification restant à faire avec accord

- ~~Lancer `inkemu inspect` sur un vrai firmware CrossPoint (test `inspect_real_crosspoint_firmware`, ignoré par défaut). Il faut d'abord télécharger un binaire tiers, ce qui exige l'accord explicite de l'utilisateur.~~ **Levé par SP1b-1** : fait le 2026-10-11, avec l'accord de l'utilisateur.
