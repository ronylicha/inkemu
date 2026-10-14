# SP1b-4a — porte de documentation — note de passation

Date : 2026-10-14. Découpage : `docs/superpowers/notes/2026-10-14-sp1b-4-decoupage.md` (plan SP1b-4a, critère 8, hygiène). Recherche : `docs/research/sp1b-4-scoping.md` (hygiène de la CI). Branche `sp1b-4a-doc-gate`.

## Résultat

- **Mesure avant** : `cargo doc --workspace --no-deps --document-private-items` rendait 93 avertissements (plus 5 lignes de résumé, une par crate qui avertit) : 83 dans `soc-esp32c3`, 4 dans `cpu-rv32`, 3 dans `emu-cli`, 2 dans `firmware`, 1 dans `boards`. Par nature : 77 plages de bits (`[9:0]`) prises pour des liens intra-documentation, 13 liens d'une documentation publique vers un élément privé, 2 balises HTML non fermées, 1 cible de lien redondante. **Après : 0** ; la commande sort en 0 avec `RUSTDOCFLAGS="-D warnings"`.
- **Convention retenue pour les plages de bits** : en code, avec des accents graves : `` `[9:0]` ``, comme les noms de registres et de champs qui les entourent (`` `PRE_DIV_CNT` ``). Les indices accolés à un identifiant le suivent : `` `imm[5]` ``, `` `shamt[5]` ``. Pas de barre oblique inverse : elle ne se lit pas dans le source et ne s'affiche pas en code.
- **Méthode** : les plages de bits ont été réécrites par une expression régulière relue (ne touche que les lignes de documentation `///` et `//!` que rustdoc signale, hors des segments déjà en code), puis `git diff --stat` et la lecture de tous les hunks : 70 lignes changées dans 22 fichiers, toutes des commentaires, aucune ligne de code. Les 16 autres avertissements ont été corrigés à la main (Edit/sed ciblé).
- **Liens vers un élément privé** (`MAP_REGISTERS`, `Ctx::deliver_due`, `Extmem::window_read`, `Extmem::flash_written`, `Extmem::invalidate`, `RtcCntl::wake`, `Timg::wake`, `PRODUCTION_V0_3`, `parse_detail`, `SLICE_CYCLES`, `SILENT_PASSES_PER_QUESTION`, `ReaderWatch`) : `--document-private-items` ne lève pas l'avertissement `private_intra_doc_links` quand l'élément qui documente est public ; les liens sont devenus du code (`` `Extmem::window_read` ``), le nom restant celui de l'élément.
- **Autres** : `"<cause>; <imperative remedy>"` dans `firmware/src/partition.rs` mis en code (rustdoc le lisait comme deux balises HTML) ; `` [`FlashRefusal::Form`](crate::flash::FlashRefusal::Form) `` de `diag.rs` réduit à `` [`FlashRefusal::Form`] `` (cible redondante).
- **Aucun code touché.** Ni test, ni clippy lancés (rien ne change à la compilation) ; `cargo fmt --all --check` passe.

## CI

- Job `check` de `.github/workflows/ci.yml`, après clippy et avant les tests : étape « Documentation without a warning », `cargo doc --workspace --no-deps --document-private-items` avec `RUSTDOCFLAGS: "-D warnings"`. Sans `--locked` : aucune autre étape cargo du job ne l'utilise. Le job enchaîne ses étapes en arrêt au premier échec, comme avant ; l'étape suit le même style.
- Le README ne liste pas les vérifications locales (seulement `cargo test --workspace`) : inchangé.

## Reste

Rien pour ce plan. La suite est SP1b-4b (enregistrement et rejeu).
