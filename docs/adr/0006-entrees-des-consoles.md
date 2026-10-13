# ADR 0006 — Entrées des consoles et hôte USB

- Date : 2026-10-13
- Statut : accepté (plan SP1b-3d)
- Amende : ADR 0004, point 1 (les entrées d'une exécution)

## Contexte

Jusqu'à SP1b-3c, les consoles de l'appareil n'allaient que vers l'hôte. Un firmware qui lit sa console (une invite, un menu, la console de CrossPoint) attendait pour toujours : l'UART n'avait pas de réception, l'USB-Serial-JTAG pas d'hôte. Or l'hôte USB fait partie de ce que voit le firmware : sans trames de début (SOF), le moniteur de connexion d'ESP-IDF et le `HWCDC` d'Arduino jugent qu'aucun hôte n'est là et jettent leur sortie (rapport `docs/research/usb-serial-jtag-host.md`, §B.3 et §B.5).
L'ADR 0004 a dit ce qui entre dans une exécution et annoncé que les octets reçus sur une console s'y ajouteraient, datés en temps d'appareil. Ce qu'un hôte envoie dépend souvent de ce que l'appareil a dit (attendre l'invite avant de taper) : une date fixée d'avance ne suffit pas, et une date lue sur l'horloge de l'hôte ferait dépendre l'exécution de la vitesse de la machine qui la fait tourner.

## Décision

1. **Deux entrées s'ajoutent aux quatre de l'ADR 0004.**
   - **L'hôte USB** de l'USB-Serial-JTAG : `absent` (pas de câble), `attached` (un câble, le port fermé : des trames, rien n'est lu) ou `open` (un programme lit et écrit le port), à la mise sous tension (`--usb-host`, `open` par défaut), puis ses changements datés.
   - **Les octets envoyés** à une console (`uart0`, `uart1`, `usb-serial-jtag`), par morceaux datés.

   Les deux entrent dans la machine par des méthodes d'`Esp32C3` (`send_console`, `set_usb_host`), qui les rangent dans les périphériques comme des événements datés : le modèle ne lit rien sur l'hôte. Une date antérieure au temps d'appareil est refusée par une erreur typée qui donne les deux instants ; rien n'est alors rangé. Une UART refuse aussi une date postérieure au dernier instant où elle peut dater sa ligne (environ 114 ans de temps d'appareil). Un morceau qui porterait au-delà d'un mébioctet les octets d'une console que l'hôte a envoyés et qui n'ont pas encore atteint l'appareil est refusé (un paquet USB vide compte pour un octet), et la machine garde au plus 16 384 changements d'hôte à venir (un changement dont la date est atteinte ne compte plus) : une interface qui fournit des entrées sans fin reçoit une erreur et non une mémoire qui grossit.

   À un même instant, la machine applique d'abord les changements d'hôte, dans l'ordre où ils ont été donnés, puis l'hôte prend le paquet IN qui l'attend, puis un paquet de l'hôte entre dans la FIFO OUT. Un paquet de l'hôte n'atteint l'appareil que 50 µs après sa date, port ouvert : un hôte qui se retire à la date d'un envoi sur l'USB-Serial-JTAG ne le remet jamais, quel que soit l'ordre des deux.
2. **La date d'une entrée est fixée par les entrées seules, jamais par la façon dont l'exécution est découpée.** Un script de console (`--console-script`) peut dater une étape de l'instant où un texte a atteint l'hôte (`wait-for`). Pour que cet instant soit exact, les canaux qu'un script attend sont « observés » (`watch_console`) : chaque octet qui atteint l'hôte sur un canal observé arrête l'appel de la machine à la frontière d'instruction où il est parti (à son instant même quand le CPU dort ; sur l'USB-Serial-JTAG, chaque paquet, à l'instant où l'hôte le prend), et le temps d'appareil est alors celui de l'octet. Un canal n'est observé que tant qu'un `wait-for` à venir le nomme. Les étapes suivantes sont datées de cet instant et remises toutes à la machine avant qu'elle reprenne. Des tests découpent les mêmes entrées en appels de tailles différentes, d'un cycle à un million (7, 997, 1 000 et 7 919 entre les deux), canaux observés ou non : la machine compare les consoles, l'empreinte de trace, le temps d'appareil et le nombre d'instructions, le script les dates de ses étapes, et un firmware de référence, exécuté sur la CI, le tout.
3. **Le terminal n'est pas une entrée.** Taper au clavier n'a de sens que si le temps d'appareil suit l'horloge murale, ce qu'inkemu ne fait pas : la commande ne relaie pas l'entrée standard. L'interface graphique (SP2) fournira ses octets par la même méthode, datés de l'instant présent de la machine, et dira qu'une telle exécution ne se rejoue qu'avec son enregistrement (SP1b-4).
4. **La console relayée reste une sortie.** `--console` choisit ce qui est montré, pas ce qui est reçu ni exécuté (ADR 0004, point 1).
5. **Un script dit jusqu'où il est allé.** Le résumé de l'exécution imprime l'état de l'hôte USB à la fin et la progression du script ; une exécution qui sortirait avec 0 sort avec 5 quand son script n'est pas allé au bout (un texte encore attendu, une étape datée après la fin). Les autres codes sont gardés : ils en disent plus sur l'exécution.

## Conséquences

- **Enregistrement et rejeu (SP1b-4)** : un enregistrement porte, en plus des entrées de l'ADR 0004, l'état de l'hôte USB à la mise sous tension et la liste datée des entrées des consoles et des changements d'hôte, telle que la machine les a reçues (pas le script, dont les dates dépendent de l'exécution). Un rejeu les remet à la machine à leurs dates, au fil de l'exécution, sans dépasser les bornes du point 1 (un mébioctet en attente par console, 16 384 changements d'hôte à venir).
- **Le script n'est pas le format des scénarios.** Sa première ligne, `inkemu-console-script 1`, le désigne ; les scénarios YAML du sous-projet 4 (boutons, écran, consoles) pourront le remplacer sans ambiguïté.
- **L'hôte d'une UART envoie au débit et dans la trame de l'UART.** Un hôte réel peut se tromper de débit ; ce cas (octets corrompus, erreurs de trame) n'est pas modélisé, et l'hypothèse est écrite dans le code.
- **Les empreintes de trace des exécutions qui écrivent sur l'USB-Serial-JTAG changent.** La ROM recopie sa console sur l'USB-Serial-JTAG ; avant SP1b-3d, l'extrémité IN était toujours libre, elle attend désormais que l'hôte prenne le paquet. Les empreintes épinglées par les tests ont été remesurées avec ce changement ([note de passation de SP1b-3d](../superpowers/notes/2026-10-13-sp1b-3d-carryover.md)) : le temps d'appareil des deux exécutions épinglées ne change pas, leur nombre d'instructions baisse.
- **Numérotation** : l'ADR suivant prend le numéro 0007.
