# ADR 0010 — Le mode utilisateur, la PMP et le banc de conformité

- Date : 2026-10-15
- Statut : accepté (plan SP1b-4f)

## Contexte

Jusqu'à SP1b-4e, le hart de l'ESP32-C3 rangeait les registres de la PMP (`pmpcfg0..3`, `pmpaddr0..15`) et
respectait leur verrou sans rien protéger (décision D18), n'exécutait qu'en mode machine (un `mret` vers `MPP` = 0
continuait en mode machine), et le groupe `pmp32` de riscv-arch-test était exclu ; le banc de conformité
(`inkemu rvtest`) était un hart RV32IMC en mode machine seul, sans PMP. Le découpage de SP1b-4 confie à SP1b-4f
l'application de la PMP, la réactivation de `pmp32` et la conformité complète.

Ce que disent les sources (rapport `docs/research/esp32c3-pmp-and-user-mode.md`, PM ci-dessous) :

- La puce a un mode utilisateur et pas de mode superviseur : `misa` vaut `0x40101104`, `mcause` 8 est « ECALL from U
  mode », `MPP` vaut 0 ou 3, `TW` interdit `wfi` en mode U ; pas de `MPRV` (TRM v1.4, registres 1.5, 1.6, 1.10 ;
  PM §1).
- Sa PMP n'est pas conforme à la spécification privilégiée : pas de priorité statique — un accès réussit s'il
  correspond à au moins une entrée active qui l'autorise — et un NAPOT d'au plus 1 Go (TRM §1.8.2 ; PM §2).
- QEMU, l'oracle du lockstep, applique la spécification (priorité statique) ; ESP-IDF v5.5.2 pose 16 entrées TOR
  verrouillées, contiguës et sans chevauchement, sur lesquelles les deux règles donnent le même verdict (PM §0, §3).
- Sail, la référence de la conformité, applique la spécification ; il ne sait pas modéliser l'absence de priorité
  statique (PM §4).
- 40 des 41 tests de `pmp32` déclarent une routine de piège du mode S ; sans mode S, l'environnement d'arch-test
  3.10.0 boucle sans fin, sur Sail comme ailleurs (mesuré, PM §5.2).

## Décision

1. **Trois cœurs**, choisis par `HartConfig::core` (`cpu_rv32::Core`, qui remplace `VendorCsrs`) :
   - `MachineOnly` : le hart RV32IMC en mode machine seul, sans PMP (le défaut, celui des tests unitaires) ;
   - `MachineUser` : **le banc de conformité**, modes machine et utilisateur, PMP de 16 entrées, les choix de la
     spécification que Sail fait quand `tools/conformance/riscof/sail` le configure (priorité statique, `MPP` non
     nominal légalisé en U, `MPRV`, `TW`, `mcounteren`, bits 5–6 de `pmpcfg` lus à 0, `pmpaddr` sur 30 bits) ;
     `inkemu rvtest`, les riscv-tests et riscv-arch-test tournent dessus ;
   - `Esp32C3` : la puce selon le TRM — modes machine et utilisateur, `misa` `0x40101104`, `MPP` réglé par son bit
     bas, `TW`, pas de `MPRV` ni de `mcounteren` ; PMP **sans priorité statique** ; ce qu'une écriture garde est ce
     que QEMU garde (octet entier, `pmpaddr` sur 32 bits).
2. **Une seule PMP pour les deux cœurs** (`cpu_rv32::pmp`) : mêmes registres, mêmes verrous, même décodage des plages
   (OFF, TOR, NA4, NAPOT ; un TOR dont la borne basse n'est pas sous la borne haute ne correspond à rien), même
   défaut (aucune entrée qui s'applique : machine autorisée, utilisateur refusé), même effet du verrou (seule une
   entrée verrouillée restreint le mode machine ; une entrée non verrouillée lui accorde tout). Diffèrent la
   combinaison des entrées, les entrées qui comptent pour le mode machine et les détails WARL ci-dessus :
   - **sur la puce**, seules les entrées verrouillées s'appliquent au mode machine (TRM §1.8.3) : une entrée non
     verrouillée ne le touche pas, ni pour accorder ni pour refuser ;
   - **sur le banc** (règles `Specification`, celles de Sail), toute entrée active participe, en mode machine comme
     en mode utilisateur : une entrée non verrouillée qui contient l'accès accorde tout au mode machine et masque les
     entrées suivantes, même verrouillées, et une entrée qui n'en contient qu'une partie le refuse.

   Une écriture R = 0 avec W = 1 laisse l'entrée telle quelle sur les deux cœurs (QEMU ; Sail configuré
   `PMP_Ignore`).
3. **Le contrôle** : chaque parcelle de 16 bits d'un fetch, avant le cache de décodage (qui ne garde que des
   décodages : une écriture de la PMP prend effet au fetch suivant sans rien invalider) ; chaque chargement et
   rangement, après le contrôle d'alignement, avant le bus. Une faute PMP a la cause d'une faute de bus (1, 5, 7) et
   l'adresse fautive dans `mtval`. Le contrôle ne coûte aucun cycle (QEMU non plus ; la puce n'est pas mesurée).
4. **Le mode utilisateur** : un `mret` avec `MPP` = 0 y entre ; un piège en sort (`MPP` garde le mode d'où il vient) ;
   un CSR dont les bits `[9:8]` dépassent le mode, `mret`, et `wfi` avec `TW` sont des instructions illégales ;
   `ecall` y a la cause 8 ; une interruption y est prise quel que soit `mstatus.MIE`.
5. **`pmp32` tourne sur le banc avec un environnement corrigé** (`tools/conformance/riscof/arch-test-env-no-smode.patch`,
   appliqué par `run-arch-test.sh` à la copie de `env/arch_test.h` que les deux modèles compilent, après vérification
   de l'empreinte de l'original) : le prologue et l'épilogue du mode S et les accès à `medeleg` sont retirés, le
   gestionnaire et la zone de sauvegarde S restent (le gestionnaire M lit les pointeurs statiques de cette zone).
   Aucun fichier de test n'est touché ; leurs parties « mode S » tournent en mode U sur les deux modèles (`MPP` = 1
   légalisé en U). Effectifs : `pmp32` = 41, total 133.
6. **Un cache des verdicts par page** évite de parcourir les 16 entrées à chaque accès, une fois qu'ESP-IDF les a
   verrouillées : pour une page dont chaque entrée applicable contient toute la page ou rien d'elle, le verdict ne
   dépend pas de l'endroit de l'accès (la règle de QEMU pour la taille de ses pages de TLB, `pmp_get_tlb_size`) ; une
   page coupée par une entrée, un accès qui franchit une fin de page, sont contrôlés entrée par entrée. Le cache est
   oublié à chaque écriture de la PMP ; un test compare ses verdicts à ceux des règles sur des configurations
   aléatoires.

## Conséquences

- Un firmware qui touche une zone que la table d'ESP-IDF interdit (un pointeur nul, une écriture en DROM, un fetch en
  DRAM) prend sous inkemu la faute qu'il prend sur la puce et sous QEMU, et son gestionnaire de panique s'exécute.
  Les firmwares de référence n'en prennent aucune : leurs empreintes de trace épinglées sont inchangées.
- Le lockstep ne gagne aucune ligne : la ligne 26 (bits R, W, X gardés par QEMU au reset) reste vraie, ses entrées
  sont éteintes et ne protègent rien en mode machine ; sa phrase sur la PMP non appliquée est révisée.
- La règle de la puce sans priorité statique n'est pas vérifiée contre un oracle : QEMU et Sail ont la priorité
  statique. Des tests unitaires la tiennent, et sur la table d'ESP-IDF les deux règles coïncident. Une sonde sur une
  puce (sous-projet 8) tranchera les lectures du TRM que PM §2 liste.
- Les riscv-tests `rv32ui` tournent désormais en mode utilisateur (l'environnement `p` y entre quand le mode existe) ;
  `rv32mi-p-pmpaddr` passe et sort de la liste d'exclusion ; le test CLI d'échec de `rvtest` utilise un ELF
  synthétique.
- Le coût mesuré : voir la note de passation de SP1b-4f.

## Alternatives écartées

- **Un mode superviseur sur le banc**, pour faire tourner `pmp32` sans correctif : la puce n'en a pas ; ce serait des
  centaines de lignes de modèle (`sstatus`, délégation, `sret`, interruptions S, `satp`) qu'aucun firmware n'exerce,
  pour vérifier en mode S ce que le mode U vérifie déjà (la PMP traite S et U de la même façon).
- **Les règles de la puce sur le banc** : Sail ne sait pas les modéliser, et les tests « priority » de `pmp32`
  échoueraient par construction.
- **La priorité statique sur la puce aussi**, comme QEMU : le TRM documente le contraire ; sur ESP-IDF les deux
  coïncident, et pour un firmware qui chevaucherait des entrées, le comportement documenté de la puce prime.
- **Un crochet de démarrage** (`RVMODEL_BOOT`) qui enjamberait les CSR absents : mesuré sur Sail, il passe `medeleg`
  mais pas le prologue S, dont les pièges arrivent hors du segment de code et font boucler l'environnement (PM §5.2).
- **Réactiver le seul test qui se passe du mode S** (`PMP-CFG-reg`) : ce n'est pas le groupe.
