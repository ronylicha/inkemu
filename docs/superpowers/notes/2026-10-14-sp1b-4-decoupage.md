# SP1b-4 — découpage décidé le 2026-10-14

Source : `docs/research/sp1b-4-scoping.md` (inventaire de l'existant, dépendances, tailles, risques). Décisions
prises sans l'utilisateur, à lui restituer au bilan ; elles sont réversibles plan par plan.

## Plans, dans l'ordre

| Plan | Jalon | Critère de la spec §10.3 | Taille |
|---|---|---|---|
| SP1b-4a | `cargo doc --workspace --no-deps --document-private-items` sans avertissement, tenu par une porte de la CI (`RUSTDOCFLAGS="-D warnings"`) | 8 (hygiène) | S |
| SP1b-4b | Enregistrement et rejeu : `inkemu run --record session.inkrec`, `inkemu replay session.inkrec` ; une crate bibliothèque pour le format ; le rejeu redonne consoles et empreinte de trace | 6 (« la même session ») | M |
| SP1b-4c | Lockstep contre QEMU de la ROM et du bootloader jusqu'à `app_main` de `hello_world`, en CI, avec une liste numérotée et sourcée des différences admises | 3 (partie CI) | L |
| SP1b-4d | Lockstep à travers l'application, et exécution locale datée sur CrossPoint (fenêtre d'environ 32 M instructions) | 3 | L |
| SP1b-4e | CoreMark dans ±15 % du score publié choisi, avec un ajustement de peu de coûts | 5 | M |
| SP1b-4f | Application de la PMP, groupe `pmp32` d'arch-test réactivé | 1 (conformité complète) | M |

## Arbitrages

- **Critère 3** : « le même état initial » n'est pas atteignable tel quel (console USB-Serial-JTAG non
  inscriptible sous QEMU, démarrage sur la PLL, eFuses, aléa de l'hôte). Le lockstep compare donc avec une
  liste numérotée de différences admises, chacune sourcée et traitée (valeur masquée, recopiée de QEMU, ou
  resynchronisation), et un modèle de timing où chaque instruction coûte le temps qu'elle coûte sous QEMU,
  sélectionné comme les autres modèles de timing (spec §4.2). Le comportement par défaut de la machine ne change
  pas.
- **CrossPoint** ne peut être ni embarqué ni téléchargé : la partie du critère 3 qui le concerne est vérifiée
  en local et datée, comme le critère 2 (SP1b-3e).
- **Sortent de SP1** : le cache fidèle à la ligne et le coût d'un défaut de cache (vers SP8 : QEMU n'a pas de
  cache, seul le matériel le vérifie ; l'affirmation contraire de l'ADR 0002 sera corrigée par une section datée) ;
  la garde de pile d'`ASSIST_DEBUG`, les permissions `SENSITIVE` et les déclencheurs de débogage (vers SP4,
  débogage) : aucun critère de SP1 ne les demande.
- Chaque plan suit le cycle habituel : spec courte, plan et code par un architecte, relectures par domaine,
  correcteurs séquentiels, CI lue dans ses journaux, merge `--no-ff`, cartographie.
