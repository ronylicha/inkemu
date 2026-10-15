# SP1b-4f — Note de passation

Plan : `docs/superpowers/plans/2026-10-15-sp1b-4f-pmp.md` (R79–R91). Spec :
`docs/superpowers/specs/2026-10-15-sp1b-4f-design.md` (L1–L12). Recherche : `docs/research/esp32c3-pmp-and-user-mode.md`.
ADR 0010 (nouvel) ; `docs/lockstep-differences.md`, révisions datées des lignes 1 et 26 et de « Ce qui n'est pas
comparé » ; `docs/registers-accepted-without-effect.md`.

## Résultat

- **La PMP de l'ESP32-C3 est appliquée** selon son TRM (§1.8) : 16 entrées, granularité 4 octets, OFF/TOR/NA4/NAPOT,
  **sans priorité statique** (un accès réussit si une entrée qui le contient l'autorise), le mode machine contrôlé
  par les seules entrées verrouillées ; ce qu'une écriture garde est ce que QEMU garde. Contrôle à chaque parcelle
  de fetch (avant le cache de décodage, qui n'a rien à invalider), à chaque chargement et rangement ; fautes 1, 5, 7
  avec l'adresse fautive dans `mtval`.
- **Le mode utilisateur de la puce est modélisé** (`misa` `0x40101104`, `MPP` {0, 3}, `TW`, `ecall` cause 8, CSR et
  `mret` illégaux en U, interruption prise en U quel que soit `MIE`).
- **Le banc de conformité** (`Core::MachineUser`) a les modes M et U et une PMP de 16 entrées avec les choix de Sail ;
  `inkemu rvtest` et les riscv-tests tournent dessus : **64 riscv-tests exécutés, 3 exclus**, épinglés dans
  `versions.env` (`RISCV_TESTS_RAN`, `RISCV_TESTS_SKIPPED`) ; `rv32mi-p-pmpaddr` passe et sort de l'exclusion,
  `rv32mi-p-zicntr` y entre à la relecture (le banc n'a pas de Zicntr, comme Sail configuré).
- **arch-test : `pmp32` réactivé, 41 tests ; total 133** (`ARCH_TEST_EXPECTED` `I=39 M=8 C=29 Zifencei=1
  privilege=15 pmp32=41`). En local le 2026-10-15 : `ARCH_TEST_ONLY=pmp32 tools/conformance/run-arch-test.sh`
  → 41 sur 41 ; mutation (le banc avec les règles de la puce) → 3 sur 41, défaite. Environnement d'arch-test corrigé
  (mode S retiré, ADR 0010), fichiers de test intacts.
- **Inchangés, revérifiés en local** : empreintes de trace 12 s `a0d221296337b295`, 40 000 001 cycles
  `fb9ea628bc25b355`, `adc_wait_panic` `b217bc3b287080ae`, `deep_sleep_timer` `80bd1c43d55b5467` ; CoreMark aux
  mêmes empreintes avant et après à `--max-time 2` (`c97883ec751f1d3a`) et `--max-time 6` (`b9d7c2623875226c`) ;
  **la fenêtre `hello` du lockstep contre QEMU** : code 0, 6 813 949 instructions, fin au reset, usages épinglés
  identiques (ligne 26 : 13 usages).
- **Coût** : non mesurable sur le poste (R90 ; médianes 27,0 s avant, 28,2 s après sur CoreMark à 2 s, une charge
  de 10 à 37 sur 16 cœurs pendant les mesures). La durée du test CoreMark en CI (68 s avant) est la référence.

## Relecture (PR #15, tête `c64fa5d`, 2026-10-15) : quatre constats corrigés

- **I1** : le nombre de riscv-tests exécutés était affiché, jamais comparé. Les effectifs sont épinglés dans
  `versions.env` à côté d'`ARCH_TEST_EXPECTED`, et `riscv_tests.rs` (job `check`, `cargo test --workspace`)
  échoue sur tout autre compte ; vérifié en le faussant d'une unité.
- **M1** : la décision 2 de l'ADR 0010 disait le mode machine contrôlé par les seules entrées verrouillées sur
  les deux cœurs ; c'est vrai de la puce, pas du banc, où toute entrée active participe (une entrée non
  verrouillée qui contient l'accès accorde tout et masque les suivantes, une correspondance partielle refuse).
  Reformulée ; un test unitaire épingle le masquage sous les deux règles.
- **M2** : le compteur de génération des verdicts de page faisait le tour en `u32`, et après 2^32 écritures de
  la PMP un verdict périmé redevenait valide. La génération part de 1, 0 marque une case vide, et au bout du
  compteur toutes les cases sont vidées ; un test force le compteur à sa dernière valeur (défait sans le
  vidage). Empreintes 12 s, 40 000 001 cycles, `adc_wait_panic` et `deep_sleep_timer` revérifiées, inchangées.
- **E1** : `inkemu_rv32imc.json` déclare `Zicntr` non pris en charge, et Sail 0.15 rend alors `cycle`,
  `time`, `instret` illégaux dans tous les modes (`zicntr_control.sail`), alors que le banc y répondait. Le
  banc est aligné sur sa déclaration (déclarer Zicntr demanderait un `time` qu'il n'a pas) ;
  `rv32mi-p-zicntr` est exclu et un test épingle son échec au cas 2. Effectifs arch-test inchangés (la
  sélection ne dépend que de l'ISA, inchangée) ; groupe `privilege` relancé en local : 15 sur 15.

## Les reports, tranchés

| Report | Décision | Héritier |
|---|---|---|
| SP1a : « les CSR PMP et les déclencheurs de débogage sont absents ; `rv32mi-p-pmpaddr` et `rv32mi-p-breakpoint` exclus ; `pmp32` désactivé ; le test CLI d'échec repose sur `pmpaddr` » | PMP : **fait** (application, `pmp32`, `pmpaddr` retiré de l'exclusion, ELF synthétique pour le test CLI). Déclencheurs : **reportés**, raison de l'exclusion de `rv32mi-p-breakpoint` mise à jour | Sous-projet 4 (débogage) : `tselect`/`tdata1`/`tdata2`/`tcontrol`, puis `rv32mi-p-breakpoint` sorti de l'exclusion |
| SP1b-4c (`CsrFile::return_privilege`, « LIMIT (SP1b-4f) : un `mret` vers `MPP` = 0 continue en mode machine ») | **Fait** : le mode utilisateur existe | — |
| SP1b-4e : « SP1b-4f : la PMP » | **Fait** | — |
| Ligne 26 du tableau : « inkemu ne fait pas respecter la PMP (SP1b-4f) » | **Révisée** (note datée) : la ligne reste vraie, les entrées gardées par QEMU sont éteintes | — |

## Décisions prises sans l'utilisateur

L1–L12 de la spec et R79–R91 du plan ; les plus engageantes :

- **La règle de la puce, pas celle de QEMU** (R80) : le TRM documente l'absence de priorité statique ; sur la
  table d'ESP-IDF les deux coïncident.
- **Deux jeux de détails WARL** (R81) : la puce comme QEMU (oracle du lockstep), le banc comme Sail (oracle de la
  conformité) ; R = 0 avec W = 1 ignoré partout, Sail configuré `PMP_Ignore`.
- **Le banc devient un cœur M+U** (R84) : les `rv32ui` des riscv-tests tournent désormais en mode utilisateur.
- **`pmp32` avec un environnement corrigé** plutôt qu'un mode S fictif sur le banc (R86, ADR 0010).
- **`Core` remplace `VendorCsrs`** dans l'API de `cpu-rv32` (R85).
- **`misa` de la puce passe à `0x40101104`** (R79, TRM registre 1.6) ; aucun firmware ne le lit.

## Laissé aux héritiers

- **Sous-projet 8 (banc, puce)** : sonder sur une puce les lectures du TRM que PM §2 liste — une entrée qui ne
  contient qu'une partie d'un accès désaligné, un NAPOT de plus de 1 Go, les bits 5–6 de `pmpcfg`, l'écriture
  R = 0/W = 1, les bits 31:30 de `pmpaddr`, le coût en cycles d'un contrôle PMP ; et en mode U, ce que fait
  `cycle` (aucun `mcounteren` au TRM).
- **Sous-projet 4 (débogage)** : `SENSITIVE` (PMS), garde-pile d'`ASSIST_DEBUG`, déclencheurs (inchangé, découpage de
  SP1b-4).
- **Tout sous-projet qui ajoute une fenêtre de lockstep** : si son firmware chevauche des entrées PMP ou pose un TOR
  vide, la première divergence avec QEMU (priorité statique, TOR vide = adresse 0) sera comparée ; la traiter alors
  par une ligne sourcée (PM §3).

## Limites de preuve

- **Vérifié en local** (poste partagé, charge 10 à 37 sur 16 cœurs) : `cargo fmt --all --check` ; `tools/ci/clippy.sh`
  (`-D warnings`, toutes cibles et fonctionnalités) sur `cpu-rv32`, `soc-esp32c3`, `emu-cli`, `lockstep` ; rustdoc
  (`-D warnings`, éléments privés) sur les mêmes ; `cargo check --target wasm32-unknown-unknown` sur `cpu-rv32`,
  `soc-esp32c3`, `lockstep` ; les tests non ignorés de ces quatre crates (`cpu-rv32` 144, 146 après la
  relecture, `soc-esp32c3`,
  `emu-cli` dont `riscv_tests` et `rvtest_cli`, `lockstep`) ; deux mutations vérifiées (contrôle sur un succès du
  cache de décodage ; règles de la puce sur le banc contre `pmp32`) ; les empreintes et la fenêtre `hello`
  ci-dessus ; le groupe `pmp32` contre Sail (image `inkemu/riscof` reconstruite avec `patch`).
- **Non exécuté en local**, laissé à la CI : `cargo test --workspace` ; les autres suites ignorées (dont le test
  CoreMark à 20 s et son empreinte `bfb20978732b673f`, les 13 autres `reference_firmware` de `run_cli`,
  `reference_boot`, `console-script`) ; la couverture ; **l'arch-test complet** (les 92 tests des autres groupes
  sur l'ISA désormais `RV32IMCUZicsr_Zifencei`) ; les fenêtres `nvs` et `nvs-ota` et les étapes de faute du
  lockstep.
- La règle sans priorité statique n'a pas d'oracle (QEMU et Sail ont la priorité statique) : elle est tenue par des
  tests unitaires et coïncide avec l'oracle sur ESP-IDF. Aucune lecture du TRM de PM §2 n'est vérifiée sur une puce.
- CrossPoint et le firmware d'origine (tests locaux, `real_firmware.rs`, fenêtre `crosspoint`) n'ont pas été
  relancés. Ils sont bâtis sur ESP-IDF, qui verrouille sa table au démarrage ; que leur version pose la même table que
  la v5.5.2 n'est pas vérifié. Héritier : la prochaine exécution locale datée de ces firmwares (sous-projet 2, qui les
  fait aller au-delà du SAR ADC).
