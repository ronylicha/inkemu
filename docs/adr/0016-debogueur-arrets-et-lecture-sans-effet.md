# ADR 0016 — Le débogueur : des arrêts qui ne changent rien, une lecture sans effet, la flash non montrée

- Date : 2026-10-15
- Statut : accepté (plan SP4a)
- Complète : ADR 0003 (temps de l'appareil), ADR 0007 (sessions), ADR 0011 (émulateur sur un fil)

## Contexte

La spec globale (§6.2) demande pause, pas à pas, registres, mémoire et points d'arrêt, puis un serveur GDB. La
machine est déterministe (§3.1) et une session se rejoue à l'identique (ADR 0007) : un débogueur qui changerait ce
que la machine exécute rendrait faux tout ce qu'il montre, et tout rejeu fait avec lui. La machine sait déjà
s'arrêter à n'importe quelle frontière d'instruction (une limite de cycles, une passe du moteur) sans rien changer.

Trois questions se posent : comment arrêter sans changer ; comment lire la mémoire alors qu'une lecture de registre
de périphérique a des effets (FIFO, bits effacés à la lecture, état du cache) ; et que montrer de la flash, qui peut
contenir des données personnelles (la NVS d'un appareil réel), alors que la règle du dépôt est qu'aucun octet de
flash n'est affiché.

## Décision

1. **Le mécanisme dans la machine, la politique hors d'elle.** `soc-esp32c3` donne `run_stopping`,
   `set_breakpoints`, `set_watchpoints`, `take_watch_hit`, `peek` et `privilege` ; la crate `debugger`, sans E/S,
   tient les bornes, les régions montrées, les refus et les vues ; `host` porte les points d'une vie à l'autre.
2. **Une seule boucle.** `run_within` et `run_stopping` partagent `run_loop`, paramétrée par des crochets qui lisent
   la machine à une frontière ou prennent ce que le bus a noté, sans rien faire d'autre. Pour `run_within`, les
   crochets sont vides et leur arrêt est de type `Infallible` : le compilateur prouve qu'ils n'arrêtent rien.
3. **Point d'arrêt** : arrêt avant l'instruction, quand le processeur va l'exécuter (`Hart::executes_next` : pas
   pendant un `wfi` qu'aucune ligne ne finit, pas quand une interruption est prise d'abord — la machine s'arrête au
   retour du gestionnaire —, pas en sommeil profond, pas sans alimentation) ; l'appel qui suit un arrêt ou un pas
   repart au-delà du point où il est, comme GDB enjambe le point dont il repart, et la machine s'arrête de nouveau
   quand elle y revient. **Point de surveillance** : note des chargements et rangements réussis du processeur (pas
   des lectures d'instruction), le premier d'une instruction, arrêt après elle ; une raison de la machine au même tour passe
   d'abord, l'accès arrête l'appel suivant avant toute instruction. La SRAM est surveillée par ses deux vues.
4. **Preuve de non-changement** : une exécution coupée à chaque point donne la même empreinte de trace, les mêmes
   cycles, instructions, temps d'appareil, registres et mémoire qu'une exécution d'un seul tenant (test de
   `soc-esp32c3`) ; une session rejouée avec deux points d'arrêt et un point de surveillance, relancée sept fois,
   donne « identique » (test de `host`). Aucune empreinte figée ne bouge.
5. **Lecture sans effet** : ROM et RAM telles quelles ; fenêtre de flash par l'entrée de MMU, sans état de cache ni
   registre de faute ; un registre de périphérique n'est jamais lu ; rien n'est signalé.
6. **La flash n'est pas montrée.** La vue mémoire ne donne aucun octet des deux fenêtres de flash (« contenu de la
   flash, non montré ») ; un point de surveillance y est refusé ; le désassemblage lit la seule fenêtre
   d'instructions, le code que le processeur exécute, et n'en donne que le texte décodé, sans encodage ni bits
   (une instruction illégale s'écrit `illegal`). Rien de ce que lit le débogueur n'entre au journal, dans un
   fichier, une session ou la sortie de la CLI ; la ligne du journal d'un arrêt de surveillance ne porte une valeur
   que d'une région permise.
7. **Les points appartiennent à l'émulateur** et à l'interface qui les garde d'un lancement à l'autre ; ils
   survivent à un reset, se posent pendant un rejeu, ne vont dans aucune session. Bornes : 64 points d'arrêt, 16
   points de surveillance de 1 à 4 096 octets, 1 024 octets de mémoire et 64 lignes de code par vue.
8. **Lecture seule** : écrire un registre, un CSR ou la mémoire changerait l'exécution sans entrer au journal ; SP4d
   tranchera pour les paquets `G`, `P`, `M` de GDB.
9. **L'état final d'une vie reste lisible** : la machine d'une vie finie est gardée jusqu'à la suivante ; elle ne
   tient aucun fichier (la flash et la carte ont été gardées avant).

## Conséquences

### Positives

- Les arrêts et la lecture servent tels quels au serveur GDB de SP4d (`Z0`, `Z2`–`Z4`, `m`, `g`) et aux scénarios
  de SP4e.
- La CLI, le lockstep et le rejeu, qui appellent `run_within`, ne paient rien : les crochets vides disparaissent à
  la compilation.
- `debugger` compile en WebAssembly : la version web (SP6) l'aura.

### Négatives

- Un accès surveillé et un arrêt de la machine au même tour sont dits en deux appels.
- La vue mémoire ne montre pas les constantes que le firmware lit dans la fenêtre de données de la flash ; SP4d
  pourra les montrer pour les sections d'un ELF que l'utilisateur fournit.
- Le code avant le PC est une meilleure supposition : avec des instructions de 16 bits, remonter est ambigu.
- Une vérification de point d'arrêt par frontière et un test par accès quand des points existent coûtent un peu de
  vitesse pendant le débogage.

## Alternatives considérées

1. **Ajouter `Breakpoint` et `Watchpoint` à `StopReason`** : chaque appelant de `run_within` (CLI, lockstep,
   rejeu, scripts, tests) aurait eu des cas impossibles à traiter ; le type `RunStop` de `run_stopping` les garde
   hors de leur vue.
2. **Remplacer l'instruction par `ebreak`** (point logiciel classique) : change la mémoire que le firmware lit et
   son empreinte, et ne marche pas en flash ; l'arrêt par frontière ne touche rien.
3. **Pas à pas jusqu'au point** (`Esp32C3::step` en boucle) : correct mais des dizaines de fois plus lent.
4. **Montrer la flash à la demande de l'utilisateur** : contraire à la règle du dépôt tant qu'aucune source ne dit
   quelles pages sont le code de l'utilisateur ; l'ELF de SP4d le dira.
5. **Lire les registres de périphérique « sans effet » un par un** : demande un second chemin de lecture dans chaque
   périphérique ; reporté, un registre est dit non lu.
