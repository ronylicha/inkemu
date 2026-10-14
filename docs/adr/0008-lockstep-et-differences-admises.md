# ADR 0008 — Lockstep et différences admises

- Date : 2026-10-14 (révisée le même jour après les revues de la politique et de la machine, puis après la seconde revue : section « Révision du 2026-10-14, seconde revue » ; étendue par SP1b-4d : section « Révision du 2026-10-14, SP1b-4d »)
- Statut : accepté (plan SP1b-4c ; plan SP1b-4d pour la traversée du reset, les fenêtres et CrossPoint)
- Spec : `docs/superpowers/specs/2026-10-14-sp1b-4c-design.md` (décisions H1–H11, et sa correction du 2026-10-14) ; spec globale §4.4, §10.2 point 10, §10.3 critère 3

## Contexte

La spec globale valide le cœur par un **lockstep** contre le fork QEMU d'Espressif : même image flash, comparaison des registres et des CSR clés après chaque instruction, première divergence rapportée avec les deux états et les 64 instructions précédentes (§4.4). Le critère 3 demande qu'il n'y ait aucune divergence jusqu'au premier accès à un périphérique que QEMU n'émule pas.

Le cadrage de SP1b-4 (§0) a établi que « le même état initial » n'est pas atteignable : QEMU et inkemu diffèrent dans la ROM et le bootloader, pas seulement par le temps (aléa de l'hôte, eFuse d'une autre puce, horloge déjà sur la PLL, registres que QEMU lit 0 ou `0xFFFFFF`, console USB qui ne se vide jamais). Et le temps doit être aligné, pas toléré : QEMU, lancé avec `-icount 3`, donne 8 ns à chaque instruction quelle que soit l'horloge ; sans le même temps, le premier tick de FreeRTOS tombe sur une autre instruction. Il fallait dire quoi comparer, comment admettre une différence sans l'absorber en silence, où la comparaison s'arrête, et comment QEMU tourne.

La première version admettait des plages entières de registres recopiées de QEMU et des masques valables toute la fenêtre ; deux revues ont montré qu'un modèle faux pouvait alors passer (une mesure de l'ADC, une cause de reset, un compteur au mauvais rythme recopiés sans contrôle). Cette révision rend chaque admission exacte et vérifiée à chaque exécution.

## Décision

1. **L'unité de comparaison est la frontière d'instruction.** Un bloc `-d cpu` de QEMU est l'état *avant* l'instruction à son PC (vérifié contre le gdbstub, oracle §1c). `Esp32C3::step` exécute une instruction puis amène la machine à la frontière de la suivante : il prend l'interruption due avant elle et traverse un `wfi` jusqu'à la ligne qui le termine. Un appel coupé après son instruction (un sommeil de plus d'une seconde, `Asleep`) laisse cette frontière en attente : l'appel suivant n'exécute rien et va jusqu'à elle ; le lockstep enchaîne ces appels (3 600 au plus, une heure de temps d'appareil). L'état lu ensuite (`pc`, `x`, `csr`) est celui que la suivante trouve. L'état de mise sous tension est comparé avant la première instruction. Le même pas servira au débogueur de l'interface (sous-projet 2) et au serveur GDB (sous-projet 4).
2. **Grandeurs comparées** : le PC, `x1`–`x31` et les huit CSR machine que les deux côtés ont et que QEMU imprime : `mstatus`, `mie`, `mip`, `mtvec`, `mepc`, `mcause`, `mtval`, `mscratch`. Les CSR superviseur et hyperviseur du cœur générique de QEMU, `mhartid`, `mstatush`, `mtval2` et `satp` ne le sont pas : la puce ne les a pas, ou `mhartid` vaut 0 des deux côtés.
3. **Le temps de QEMU est un modèle de temps d'inkemu.** Le modèle `qemu-icount` (`Timing::QemuIcount`) donne exactement 8 ns à chaque instruction exécutée, aucune à la prise d'une interruption ni à l'attente, ni à une instruction dont l'accès mémoire lève une faute : QEMU la rend à son compteur d'instructions (`cpu_restore_state_from_tb`), alors qu'il compte `ecall`, `ebreak` et une instruction illégale. Après n instructions, le temps d'appareil est le nombre entier de cycles de la base de temps (160 MHz) au plus égal à n × 8 ns, 32 cycles toutes les 25 instructions. La latence du contrôleur d'interruptions vaut 0 sous ce modèle, 4 cycles sous le modèle par défaut (TRM §1.5.3.1), et `SocConfig::interrupt_latency_cycles` peut la fixer. Le modèle par défaut ne change pas.
4. **Toute différence admise est une ligne numérotée** de `docs/lockstep-differences.md`, avec sa source, sa mesure et ses paramètres exacts, et de `lockstep::DIFFERENCES` ; un test vérifie que les deux tableaux sont le même, paramètre par paramètre. Les traitements, par ordre de préférence :
   - **(i) paire de valeurs** : des bits d'un CSR valent exactement une valeur dans QEMU et une autre dans inkemu, les autres bits étant comparés ; une paire peut ne valoir que jusqu'à la première écriture du CSR. Le bit `mip.MEIP`, qu'inkemu ne montre pas, est tenu à la ligne que la matrice de QEMU calcule dans l'état du contrôleur d'inkemu, avec le retard de trois instructions que QEMU montre après un rangement du seuil ;
   - **(ii) copie de bits** : chaque registre est nommé avec les bits mesurés différents ; à un chargement d'un mot, les bits hors masque doivent être égaux et ceux du masque sont pris à QEMU, la valeur d'inkemu restant dans l'historique. Un registre de masque 0 admet l'accès à un registre que QEMU n'a pas sans rien copier. Une copie peut exiger une valeur de QEMU (une constante de son modèle) ou une condition (la commande SPI1 qui a précédé). **(ii) compteur tenu** : chaque côté lit son propre modèle ; l'écart entre les deux, fixé à la première lecture après une origine, ne peut bouger que comme la ligne le source, et une lecture dans la borne prend la valeur de QEMU, au-delà c'est une divergence. **(ii) horloge lente** : un temporisateur que QEMU n'a pas vaut, dans inkemu, les cycles de sa fréquence déclarée au temps de l'appareil de sa capture ;
   - **(iii) resynchronisation** à un PC d'ancrage ;
   - **(iv) réglage `QemuCompat`** de la machine, désactivé par défaut et qu'aucun frontal de l'émulateur n'allume, quand la différence change le flot de contrôle et qu'aucun des trois premiers ne suffit. La machine compte ce que chaque réglage a fait (`CompatActs`).

   **Chaque exécution vérifie le tableau** (`lockstep::Usage`, `Lockstep::check_usage`). L'outil enregistre ce que chaque ligne a fait : registre, bits pris à QEMU, mouvement de l'écart d'un compteur, actions d'un réglage. Il l'imprime et sort en échec (code 5) dans trois cas : une ligne utilisée hors de ce qu'elle déclare, un compteur au-delà de sa borne, ou une partie déclarée jamais utilisée. Une partie, c'est une ligne entière, un registre d'une ligne, ou un bit d'un masque. Une fenêtre coupée par une limite d'instructions liste ce qu'elle laisse inutilisé sans échouer. La CI épingle en plus le nombre d'usages de chaque ligne.

   Sur l'image de référence, 24 lignes suffisent : 4 lignes (i), 19 lignes (ii) dont une avec un réglage (iv), et une ligne (iv) seule. Aucune n'a eu besoin d'une resynchronisation. Les deux réglages : le compteur de cycles compte la base de temps (ligne 5), pas d'alarme sur un compteur arrêté (ligne 24). Aucun ne remplace la valeur que lit un modèle d'inkemu.
5. **La fenêtre** s'arrête, sans que ce soit une divergence, au premier reset des deux côtés à la fois (QEMU revient à `0x40000000`, il réinitialise toute la machine ; la machine compte ses resets), au premier accès à une adresse de la fenêtre des périphériques qu'aucun périphérique de QEMU ne couvre, sauf si une ligne nomme ce registre, ou à la limite `--max-instructions`. Un reset d'un seul côté est une divergence. La bibliothèque rend aussi la fin de la trace ; pour l'outil, une trace qui s'arrête avant la fenêtre est une capture coupée, donc une erreur (code 2), qu'elle vienne d'un fichier ou de QEMU. Les périphériques de QEMU sont pris à l'octet près dans son arbre mémoire aplati (`info mtree -f`, relevé le 2026-10-14) : tout le reste, qui tombe dans sa zone fourre-tout lue 0, est absent, qu'il s'agisse d'un bloc du SVD que QEMU n'a pas, d'un registre au-delà de ce qu'il modélise d'un bloc, ou d'un registre que le SVD ne décrit pas (la radio : FE, FE2, NRX). Les lignes nomment les registres de ces blocs que l'image de référence touche (IO_MUX, SPI0, APB_CTRL, SENSITIVE, ASSIST_DEBUG, le maître I²C analogique, la radio, sept registres d'APB_SARADC) ; une mesure de l'ADC, une batterie, les boutons sur une échelle de résistances terminent la fenêtre. Cette règle est la correction du 2026-10-14 de H4.
6. **Le lecteur de la trace** lit le texte de `-d cpu,nochain,int` au fil d'un tube, une ligne à la fois et bornée (512 octets par ligne, 40 lignes par bloc), et refuse un bloc commencé dans un autre ou une grandeur donnée deux fois. Il écarte et compte deux sortes de blocs qui ne sont pas l'état d'une instruction : un bloc réimprimé à l'identique quand le budget d'instructions de QEMU se recharge, une fois de suite au plus (jamais deux dans la fenêtre) — un deuxième est comparé comme une instruction exécutée, et un saut sur lui-même est reconnu sur les bits de l'instruction et l'état de QEMU, pas sur le décodeur d'inkemu ; et un bloc que QEMU a imprimé en y entrant puis dont il a pris l'interruption avant d'exécuter l'instruction (le `-d int` qui le suit a pour `epc` son propre PC). Une trace sans aucun bloc est une erreur. Le rapport garde une fenêtre glissante des 64 dernières instructions.
7. **Le rapport d'une divergence** donne sa cause (des grandeurs, ou le reset d'un seul côté), l'instruction, l'interruption prise par chaque côté, l'état d'avant et les deux états d'après pour chaque grandeur, les différences marquées `<<` et les admises `(admitted)`, les 64 instructions précédentes avec chaque valeur prise à QEMU (les deux valeurs et la ligne), et la ligne du tableau qui s'applique presque : celle qui nomme le registre accédé, puis celle qui tient le CSR lu ou une grandeur qui diffère, puis celle qui nomme un registre de la même page de 4 Kio.
8. **QEMU ne tourne que dans l'image Docker épinglée** `espressif/idf:v5.5.2`, celle des firmwares de référence, jamais sur l'hôte. L'étiquette de l'image et la build de QEMU (`QEMU_ESP_BUILD`) sont dans `tools/conformance/versions.env`, qu'un test relie aux valeurs par défaut de l'outil. `docker run --rm --pull=never --network none`, point d'entrée le binaire QEMU de l'image, l'image flash (une flash entière de la taille du profil) montée en lecture seule par `--mount` et ouverte avec `snapshot=on`, la trace écrite dans un tube nommé d'un répertoire temporaire, Docker dans son propre groupe de processus pour qu'un Ctrl-C n'atteigne que l'outil. Le conteneur et le répertoire sont supprimés quoi qu'il arrive (erreur, panique, Ctrl-C) ; QEMU a 120 s pour ouvrir sa trace ; une fin de trace est QEMU qui s'arrête, ce qui est une erreur. QEMU écrit environ 80 000 instructions par seconde dans le tube, sept fois ce que donne la sortie standard à travers Docker ; la comparaison en traite environ 40 000 par seconde (85 s pour la fenêtre). QEMU tourne avec `-icount shift=3,sleep=off` : avec `sleep=on`, sa valeur par défaut, son horloge virtuelle suit celle de l'hôte pendant que le CPU attend, et deux exécutions de la même image différaient de 21 instructions ; avec `sleep=off`, elle saute à l'échéance suivante, comme le temps d'appareil d'inkemu. Lancer QEMU demande un hôte Unix ; `--trace` fonctionne partout.
9. **L'outil est à part.** La crate `lockstep` (lecteur, comparateur, politique, tableau, usage, rapport) ne fait aucune entrée-sortie sur l'hôte et se compile pour WebAssembly ; le binaire de développement `inkemu-lockstep` (`crates/lockstep-cli`, `publish = false`) lance QEMU, construit la machine du profil sous `lockstep::table::configure`, imprime le résumé (instructions comparées, blocs écartés, interruptions, lignes de trace ignorées, usage de chaque ligne, fin de fenêtre) Il sort avec l'un de ces codes :

   - 0 ;
   - 1, divergence, reset d'un seul côté compris ;
   - 2, erreur ;
   - 3, fin ou usage autre qu'annoncé par `--expect-compared`, `--expect-end` ou `--expect-usage` ;
   - 4, inkemu s'est arrêté de lui-même ;
   - 5, partie de ligne inutilisée, ou ligne utilisée hors de ce qu'elle déclare ;
   - 130, Ctrl-C.

   Deux options cachées mettent une faute exprès dans inkemu une fois N instructions comparées. `--perturb-after N` change un bit de `t0`, ce qui éprouve le chemin d'un registre différent jusqu'au rapport. `--perturb-systimer-after N` ajoute une unité à la prochaine lecture d'un compteur du SYSTIMER, ce qui éprouve une ligne. La CI vérifie les deux contre le vrai QEMU. L'outil est absent du binaire `inkemu` et de toute distribution.

## Conséquences

- **Le critère 3 est tenu pour l'image de référence, au-delà de ce que la spec demandait** : de la première instruction de la ROM au reset que demande `esp_restart` à la fin du compte à rebours, 3 402 169 instructions comparées, 1 112 interruptions prises par les deux côtés à la même instruction, `app_main` vers l'instruction 2,396 millions et le premier tick vers 2,423 millions. Deux exécutions vivantes donnent le même compte et le même usage des lignes. Le job `reference-boot` de la CI épingle ce compte, la fin de fenêtre « reset des deux côtés », imprime l'usage, et vérifie qu'une faute mise exprès dans inkemu est trouvée.
- **Usage des lignes dans la fenêtre** (ce qui a été mesuré le 2026-10-14) :

  | Ligne | Usages | Ce qui a servi |
  |---|---|---|
  | 1 | 2 781 658 | `mstatus.MPP`, paire (0, `0x1800`) |
  | 2 | 3 402 170 | `mie`, paire (`0x0EEE`, 0), jamais écrit |
  | 3 | 15 | `mtvec.MODE`, jusqu'à l'écriture par la ROM |
  | 4 | 94 494 | `mip.MEIP` de QEMU égal à sa matrice calculée sur inkemu, dont 3 336 dans le retard |
  | 5 | 566 442 | 283 221 lectures sur la base de temps (le réglage) et 283 221 lectures du compteur d'inkemu tenues à un cycle de leur écart d'origine (mouvement maximal 1) |
  | 6 | 3 165 | UART0 : `STATUS` 3 155, `CONF0` 1, `FSM_STATUS` 5, `CLK_CONF` 4 |
  | 7 | 20 | SPI1 : `W0` après `0x5A` 1, après `0x05` 1, et quatre registres de reset |
  | 8 | 81 | SPI0 : 13 registres, dont 6 admis sans copie |
  | 9 | 92 | RTC_CNTL : 19 registres, dont `RESET_STATE` bits 12–13 (20), et `TIME_LOW0` (5) tenu à l'horloge lente au temps de l'appareil, écart 0 |
  | 10 | 67 | eFuse : 5 mots |
  | 11 | 284 | IO_MUX : 22 broches |
  | 12 | 147 | regi2c : 3 registres |
  | 13 | 14 | radio : 4 registres admis sans copie |
  | 14 | 13 | TIMG0 et TIMG1 : 9 registres |
  | 15 | 2 220 | 2 218 lectures des compteurs d'inkemu (compteur 0 : 18, compteur 1 : 2 200, mouvement maximal 1 par lecture) et 2 copies de `VALUE_VALID` |
  | 16 | 21 | APB_CTRL : 4 registres |
  | 17 | 1 604 | `RNG.DATA` |
  | 18 | 48 | APB_SARADC : les 7 registres |
  | 19 | 7 033 | `EP1_CONF` bit 1 |
  | 20 | 46 | SYSTEM : 12 registres |
  | 21 | 77 | SENSITIVE : 20 registres, dont 12 admis sans copie |
  | 22 | 24 | EXTMEM : 6 registres |
  | 23 | 14 447 | ASSIST_DEBUG : 7 registres, dont 6 admis sans copie |
  | 24 | 1 | une alarme retenue |

- **Une modification du modèle qui change une instruction, un registre, un bit hors des masques, un compteur au-delà de sa borne ou un instant d'interruption est détectée** et rapportée avec les deux états : au niveau unitaire sur des traces synthétiques écrites par une machine (test d'acceptation 2, une instruction changée), et contre le vrai QEMU dans la CI (une faute mise exprès dans inkemu, trouvée à l'instruction où elle est mise).
- **Les différences admises disent ce que QEMU n'est pas** : 18 lignes sur 24 sont des registres de périphériques que QEMU lit autrement que la puce. Le lockstep prouve le cœur, le décodage, les exceptions, les interruptions et leur instant, la MMU et le cache vus du programme ; il ne prouve pas les bits de ces registres, que seuls un appareil ou le banc du sous-projet 8 trancheront. Ce que les copies et les réglages laissent hors de la comparaison est dit ligne par ligne dans le tableau : le moniteur de pile désactivé après la première interruption, la PLL que le bootloader croit déjà sélectionnée, l'interruption de tick précoce, et l'origine des compteurs (lignes 5 et 15), dont seule l'évolution est tenue.
- **Ce qui n'est pas comparé** : les CSR que QEMU n'imprime pas (PMP, `mcountinhibit`, `mpcer`, `mpcmr`) ; un rangement, sinon par les chargements qui le relisent ; l'état d'un bloc préempté par une interruption, sinon par le premier bloc du gestionnaire (`mepc`, `mcause`) ; le chemin du composant que le réglage de la ligne 24 remplace, pour tout comparateur sur un compteur arrêté.
- **Ce qui reste ouvert** : le mécanisme du retard de trois instructions de `mip.MEIP` dans QEMU n'est pas identifié (la ligne 4 l'admet tel que mesuré) ; l'aléa de QEMU (`srand(time(NULL))`, semé à la seconde de l'hôte) ne peut pas être fixé par une option. La comparaison n'en dépend pas, puisqu'inkemu prend chaque valeur de QEMU. La longueur de la fenêtre n'en dépend pas non plus, d'après la mesure : trois captures à des secondes différentes comparent le même nombre d'instructions avec le même usage, et la CI épingle les deux. Pour un autre firmware, rien ne le garantit (ligne 17).
- **La traversée d'un reset et CrossPoint** (sous-projet 1b-4d) partiront de cet outil : QEMU réinitialise toute la machine, la puce ne réinitialise que le CPU (oracle, corrections), et CrossPoint, qui ne peut être ni distribué ni téléchargé, se compare en local ; ses accès à l'ADC et à SPI2 termineront la fenêtre, ou demanderont des lignes avec leur preuve.
- **Les réglages `QemuCompat`** sont une porte que seul le lockstep ouvre ; toute nouvelle option devra être une ligne (iv) justifiée comme les lignes 5 et 24, et comptée par ce qu'elle fait. Un réglage ne remplace jamais l'arithmétique d'un modèle : il peut changer la base de temps qu'un modèle compte, ou un comportement de QEMU qui change le flot de contrôle (une alarme), jamais la valeur qu'un modèle lit.

## Révision du 2026-10-14, seconde revue

Une seconde revue a trouvé que le lockstep passait encore avec un inkemu faux à trois endroits. Les corrections :

- **Les compteurs sont ceux d'inkemu (lignes 5 et 15).** La première révision faisait lire à inkemu des compteurs refaits selon les formules de QEMU (`systimer_counts_as_qemu`, `cycle_counter_as_qemu`). Le lockstep comparait alors QEMU à une copie de QEMU, et le SYSTIMER d'inkemu (capture, `VALUE_VALID`, moitiés haute et basse) n'était comparé à rien. Ces réglages sont retirés :
  - Le SYSTIMER lit ses propres compteurs et son reset par `SYSTEM.PERIP_RST_EN0` revient.
  - Le compteur de cycles est celui de la puce. Le réglage `cycle_counter_on_time_base` ne lui donne que la base de temps à compter : c'est l'horloge que les deux comptent sous `qemu-icount`.
  - Le lockstep tient l'écart entre la valeur d'inkemu et celle de QEMU. Pour le compteur de cycles, cet écart reste à un cycle de celui de la première lecture, borne sourcée par les deux parties entières du même temps. Pour le SYSTIMER, il ne descend que d'une unité par mise à jour que QEMU a pu faire entre deux lectures, et ne monte jamais, borne sourcée par la troncature de `esp_systimer_update_counter`.
  - Mesure : mouvement maximal de 1 sur les deux lignes, sur toute la fenêtre.
  - Le `VALUE_VALID` toujours 1 de QEMU, face au 0 de reset de la puce avant le premier `UPDATE`, est une copie de bit mesurée.
  - Limite dite dans le tableau : l'origine d'un compteur n'est pas comparée.
- **Le temporisateur RTC est tenu (ligne 9).** La copie entière de `TIME_LOW0` sous la condition « QEMU lit 0 » ne vérifiait rien.
  - Terminer la fenêtre à sa lecture l'aurait arrêtée avant `app_main`.
  - QEMU doit donc lire 0, et inkemu les cycles à 136 kHz au temps de l'appareil de la dernière capture, à un cycle près. Mesure : écart 0 sur les cinq lectures.
- **Chaque registre et chaque bit d'une ligne doivent servir.** `Usage::check` contrôle chaque entrée de registre et chaque bit de masque, pas seulement la ligne : une admission morte dans une ligne utilisée échoue. Une fenêtre coupée par `--max-instructions` liste ce qu'elle laisse inutilisé sans échouer. La CI épingle le nombre d'usages de chaque ligne (`--expect-usage`).
- **Le retard de `MEIP` (ligne 4)** ne s'ouvre plus qu'après un rangement du seuil qui fait tomber la ligne. Une lecture de `mip` dans un registre prend le `MEIP` que QEMU montrait.
- **L'outil.** Une trace qui finit avant la fenêtre est une erreur. La bibliothèque offre la vérification de l'usage (`Lockstep::check_usage`). Une seconde faute exprès, dans le modèle du SYSTIMER, doit diverger dans la CI.
- **Tests ajoutés.** Le reset d'inkemu seul (divergence), un programme de sommeil profond dans l'équivalence de `step` et `run_within`, la lecture de `mip`, un compteur dont l'écart monte ou descend de deux pour une mise à jour (divergence).

## Révision du 2026-10-14, SP1b-4d

Spec : `docs/superpowers/specs/2026-10-14-sp1b-4d-design.md` (décisions J1–J9) ; plan :
`docs/superpowers/plans/2026-10-14-sp1b-4d-lockstep-application.md` (décisions R30–R41). Le
lockstep traverse désormais le reset qu'`esp_restart` demande, compare la seconde vie, couvre deux
autres firmwares de référence en CI et CrossPoint 1.6.5 en local.

- **La référence du reset est la puce (J1, R30).** `esp_restart` finit par
  `esp_rom_software_reset_cpu(0)` (lu dans l'image épinglée) : `SW_PROCPU_RST`, cause
  `RTC_SW_CPU_RST` (0x0C), un reset du seul CPU (TRM §6.1). C'est ce qu'inkemu modélise depuis
  SP1b-3a ; rien n'y change. QEMU réinitialise toute sa machine et donne la même cause.
- **La traversée ne resynchronise rien (J2, R31).** Au reset des deux côtés après la même
  instruction, la comparaison continue ; rien de la machine n'est touché, et ce que le lockstep
  savait de QEMU (CSR écrits, écart des compteurs, commande SPI1, retard de `MEIP`) repart comme à la
  mise sous tension. Ce que les deux resets laissent différent est une ligne mesurée, de trois
  traitements nouveaux :
  - **(i) gardé au reset** (lignes 25 et 31, R32) : QEMU garde ses registres entiers, `mtvec`,
    `mepc` et `mstatus.MPIE`, inkemu prend ses valeurs de reset (`Hart::reset` ; pour les registres
    entiers, une hypothèse sur la puce, que seul un appareil tranchera). Chaque grandeur différente
    est tenue à ces deux valeurs exactes jusqu'à ce qu'une instruction l'écrive ; toute lecture
    diverge (`Cause::ReadKeptAtReset`), sauf un rangement d'un mot, qui porte la paire en mémoire, et
    le chargement qui la rend exactement. Mesure : aucune instruction des deux fenêtres ne calcule
    avec une telle valeur. Une copie de la valeur de QEMU aurait caché tout usage.
  - **(ii) CSR gardé au reset** (ligne 26, R33) : la PMP de QEMU garde les bits R, W, X de chaque
    entrée, la spécification privilégiée n'exige que A et L effacés. Une lecture doit rendre chez
    QEMU la valeur d'inkemu plus les bits qu'inkemu avait avant le reset, dans le masque mesuré.
  - **(ii) copie après reset** (lignes 27, 28 et 32, R34) : registres que les deux resets laissent
    différents, copiés seulement après le reset ; dans la première vie, ces bits restent comparés.
- **Fenêtres nommées (J3, R35, R36).** `lockstep::WINDOWS` : un firmware, les resets traversés,
  une borne éventuelle (aucune fenêtre n'en a besoin) et ses lignes, chacune utilisée entièrement à
  chaque exécution. Un firmware qui montre d'autres registres ou d'autres bits reçoit ses lignes.
- **Fenêtres mesurées le 2026-10-14** (QEMU `esp_develop_9.2.2_20250817`) :

  | Fenêtre | Comparées | Fin | Interruptions | Plus grands mouvements |
  |---|---|---|---|---|
  | hello (CI) | 6 813 949 | reset, 1 traversé | 2 224 | 1 (lignes 5 et 15) |
  | nvs (CI) | 7 680 555 | reset, 1 traversé | 2 224 | 1 (lignes 5 et 15) |
  | nvs-ota (CI) | 7 701 890 | reset, 1 traversé | 2 224 | 1 (lignes 5 et 15) |
  | crosspoint (local) | 31 088 420 | `APB_SARADC.APB_TSENS_CTRL` (`0x60040058`) à `0x4214917C` | 0 | 1 (lignes 5 et 39) |

  La CI épingle le compte, la fin et l'usage de chaque ligne des trois premières, et vérifie qu'une
  faute mise dans `t0` après le reset (`--perturb-after 4000000`) est trouvée à l'instruction où elle
  est mise, un reset traversé.
- **CrossPoint en local (J6, R39, R41).** `crates/lockstep-cli/tests/crosspoint.rs`, ignoré, lit
  `INKEMU_CROSSPOINT_X3_X4`, vérifie l'empreinte épinglée (`8eccf614…0385764`), compose la flash
  comme `inkemu run --firmware` et compare la fenêtre `crosspoint`. Exécuté une fois le 2026-10-14 :
  la ROM, le bootloader, l'écriture d'`otadata` et le début de l'application, jusqu'à la lecture du
  capteur de température, que QEMU n'a pas. Les lignes 34 à 41 y sont mesurées. C'est le critère 3
  sur un vrai firmware de la X4, jusqu'au premier périphérique que QEMU n'émule pas.
- **Les applications composées n'ont pas de fenêtre (J5, R37).** `adc_wait_panic`, `uart_select`,
  `usb_serial_jtag_console` et `deep_sleep_timer` s'arrêtent dans leur démarrage (3,8 à 4,0 millions
  d'instructions) au premier tick de FreeRTOS, qui tombe pendant que le CPU exécute : QEMU le prend
  deux instructions après inkemu (dans `hello` et `nvs`, chaque tick tombe dans un `wfi`).
  `usb_serial_jtag_echo` s'arrête à 2 525 511 : QEMU montre `MEIP` alors que sa matrice, calculée sur
  le contrôleur d'inkemu, ne lève pas la ligne ; la cause n'est pas identifiée. Ni l'un ni l'autre
  n'est admis : ce serait une ligne sur l'instant d'une interruption, ce que le lockstep doit prouver.
- **Pas de changement de QEMU (J7, R38).** `20260417` corrige des interruptions perdues quand `MIE`
  vaut 0 ; aucune fenêtre ne l'a rencontré, et les deux artefacts ci-dessus sont d'une autre nature.
- **Rien ne change par défaut (J8).** `soc-esp32c3` et `cpu-rv32` ne changent pas ; l'empreinte de
  12 s de `flash_hello.bin` reste `a0d221296337b295`.
