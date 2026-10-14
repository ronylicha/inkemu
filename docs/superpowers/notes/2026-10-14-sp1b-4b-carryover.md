# SP1b-4b — enregistrement et rejeu des sessions — note de passation

Date : 2026-10-14. Spec : `docs/superpowers/specs/2026-10-14-sp1b-4b-design.md` (décisions G1 à G10). Plan : `docs/superpowers/plans/2026-10-14-sp1b-4b-record-replay.md` (décisions R1 à R13). ADR : `docs/adr/0007-format-des-sessions.md`. Branche partie de `sp1b-4b-record-replay` (`c590b3f`).

## Résultat

- **Journal de la machine (G1).** `soc-esp32c3/src/inputs.rs` : `AcceptedInput` (octets envoyés à une console, changement d'hôte USB, chacun daté), `InputJournal` (l'hôte USB à la mise sous tension, les entrées dans l'ordre où la machine les a acceptées). `Esp32C3::send_console` et `set_usb_host` journalisent après toutes leurs vérifications : une entrée refusée n'y est pas. Le journal est borné sur toute l'exécution par les bornes de l'ADR 0006 (1 Mio par console, chaque morceau comptant au moins un octet ; 16 384 changements d'hôte) ; au-delà, il se vide, se dit incomplet (`is_complete`), et la machine accepte quand même les entrées.
- **Crate `session` (G2–G4).** Sans entrée-sortie sur l'hôte (cinquième copie du `clippy.toml` d'interdiction, `host_io_ban.rs` passé à cinq crates), compilée pour wasm32 (étape WebAssembly de la CI). `Session::to_bytes` / `from_bytes` (format `inkemu-session 1`, binaire, flash compressée par `flate2`, déjà dépendance de l'espace de travail, dont le moteur est en Rust pur) ; `Outcome`, `End`, `ConsoleDigests`, `Divergence` ; `Limits::run_limits` et `Limits::end_of`, partagés par le rejeu de la crate et celui de la CLI ; `Replay::start` / `step` / `finish` / `into_machine`. `boards::builtin_toml` donne le texte d'un profil livré.
- **CLI (G5, G6).** `inkemu run --record <fichier>` ; `inkemu replay <fichier> [--console] [--rom-dir]` ; code de sortie 6 (`EXIT_DIVERGED`, `REPLAY_EXIT_CODES_HELP`). `execute_with_script` prend un `RunExtras` (script et empreintes des consoles) : les empreintes reçoivent tout ce que chaque console émet, et ce que la fin vide. `emu-cli/src/record.rs` (`Recorder`, `replay_options`, `verdict`) et `emu-cli/src/session_file.rs` (lecture bornée par `read_regular_file`, écriture par `.<nom>.tmp` et renommage, mode `0600`).
- **Preuve (G7, G9).** `crates/session/tests/replay.rs` : un firmware écrit en instructions vit trois vies (écho sur UART0 de ce qu'UART0 et l'USB-Serial-JTAG reçoivent ; reset logiciel après la première ligne ; sommeil profond sur le timer RTC après la deuxième ; sommeil sans source après la troisième), l'hôte USB passe d'`attached` à `open`, puis `absent`, puis `open`. L'enregistrement remet ses entrées au fil de l'exécution, comme un script ; le rejeu les remet toutes à la mise sous tension. Résultat identique (fin, temps d'appareil, cycles, instructions, empreinte de trace, flash finale, trois consoles) en une tranche et en tranches de 1, 7, 997 et 7 919 cycles, relu depuis ses octets, deux fois ; une session coupée à 3 ms, et une coupée pendant le sommeil profond, rejouent jusqu'à leur coupure ; un octet d'entrée changé ne fait différer que la console UART0. Côté CLI, `record.rs` enregistre puis rejoue par `execute_with_script` (identique, divergence nommée, coupure par Ctrl-C à la mise sous tension). Test `x4_records_the_uart_select_reference_firmware_typed_into_and_replays_it_identically` (`run_cli`, ignoré, job `reference-boot`, effectif 11 → 12).
- **Documents (G8, G10).** ADR 0007 ; README (anglais puis français) : section « Recording and replaying a run », code 6, feuille de route ; bloc d'aide de `run` (option `--record`, code 2 qui couvre la session) ; plan.

## Décisions prises pendant l'écriture du plan

Voir le tableau du plan (R1 à R13). En bref :
- le rejeu remet toutes les entrées à la mise sous tension, dans l'ordre du journal (le contrat de la machine le permet, les tests le vérifient) ;
- les bornes d'une session sont des totaux sur l'exécution, égaux aux bornes d'ADR 0006 sur ce qui est en attente ; une exécution qui les dépasse n'est pas enregistrée (code 2, 3 après `--strict-mmio`) ;
- pas d'empreinte du fichier entier : une entrée modifiée diverge (code 6) au lieu d'être refusée ;
- une exécution coupée par l'hôte (Ctrl-C, lecteur parti, console impossible à écrire, y compris au vidage final) est enregistrée avec l'instant de sa coupure, qui devient une limite de temps du rejeu ; tout arrêt sur une limite à cet instant ou après est la coupure ;
- `--record` active l'empreinte de trace ;
- la console affichée n'est pas enregistrée : `replay --console` la choisit ;
- un rejeu coupé par l'hôte ne compare rien et sort avec 2 ;
- la ROM du rejeu est celle de la révision du profil, chargée comme par `run`, refusée si son SHA-256 n'est pas celui de la session ;
- le fichier est écrit par un temporaire et un renommage, en `0600`, et `--record` ne peut pas désigner un fichier que l'exécution lit ;
- format binaire, petit-boutiste, ordre fixe des champs ; la comparaison donne la première grandeur dans l'ordre de G3 avec ses deux valeurs, puis nomme les autres ;
- le test de référence utilise `uart_select` et le script du test de référence de `console-script` ;
- le profil voyage en texte, reconstruit par `DeviceProfile::from_named_toml` ;
- un morceau vide envoyé à une UART est journalisé (coût 1) ; les aides de sommeil profond des tests de la machine passent dans `test_support`.

## Exécuté localement (hôte de l'architecte)

`cargo fmt --all` ; `tools/ci/clippy.sh -p soc-esp32c3 -p boards -p session -p console-script -p firmware -p emu-cli --all-targets --all-features -- -D warnings` ; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items -p soc-esp32c3 -p boards -p session -p emu-cli` ; `cargo check --target wasm32-unknown-unknown -p soc-esp32c3 -p boards -p session` ; `cargo check --workspace --all-targets` ; `cargo test -p soc-esp32c3 --lib -- inputs machine::tests::the_machine_journals machine::tests::a_deep` (10) ; `cargo test -p boards --lib builtin` (3) ; `cargo test -p boards --test host_io_ban` (3) ; `cargo test -p session` (17 unitaires, 7 d'intégration) ; `cargo test -p emu-cli --lib` (185) ; `cargo test -p emu-cli --test run_cli -- replay record help readme no_session` (10, dont le contrôle du bloc d'aide du README) ; `cargo test -p emu-cli --test run_cli -- --ignored --list` : 12 tests `reference_firmware`, 28 autres. Rien d'autre : ni suite complète, ni suites ignorées, ni couverture, ni Docker, ni ESP-IDF.

## Reporté, par héritier

- **SP1b-4c/4d (lockstep)** : partir d'une session pour comparer deux machines ; le journal et `Replay::start` donnent la machine de départ et ses entrées.
- **SP2 (interface)** : enregistrer et rejouer depuis l'interface (`session::Replay`, `Recorder` à reprendre hors de la CLI) ; le clavier de l'interface fournira ses octets par `send_console`, journalisés comme les autres ; les boutons et la carte SD rejoindront le journal et changeront le numéro de version du format. Un enregistrement périodique ou à la demande, en cours d'exécution, n'existe pas.
- **SP4 (scénarios)** : un scénario pourra produire une session ; un instantané d'état (reprendre au milieu d'une exécution) n'est pas demandé par la spec et n'existe pas.
- **SP6 (web)** : lire le fichier dans le navigateur et le donner à `Session::from_bytes`.
- **Hors périmètre, nommé** : rejouer une session enregistrée par un autre modèle (une correction du modèle fait diverger les sessions d'avant, ce qui est voulu) ; la compression des entrées (seule la flash est compressée).

## Limites de preuve

- Le test de référence (`uart_select` enregistré puis rejoué) n'a pas été exécuté par l'architecte (ni ROM réelle ni image de référence sur son hôte) : il est vérifié par la première exécution de la CI. Les valeurs qu'il compare viennent de l'exécution elle-même (console, empreinte de trace) : il n'épingle rien.
- Le chemin de la CLI avec la vraie ROM (`run --record` puis `replay`) n'est tenu que par ce test ; sans ROM, il est tenu par `record.rs` (la même boucle `execute_with_script`, une ROM d'instructions) et par les refus de `run_cli` qui précèdent le chargement de la ROM.
- La remise anticipée des entrées repose sur le contrat de la machine (ADR 0006) : vérifiée par le firmware de trois vies, à travers un reset logiciel, un sommeil profond et quatre états de l'hôte USB ; un modèle de périphérique qui dépendrait de l'instant de la remise ferait diverger un rejeu, et les tests le montreraient.
- L'écriture du fichier n'est pas synchronisée au niveau du répertoire après le renommage (même limite que le fichier de flash, ADR 0004).
