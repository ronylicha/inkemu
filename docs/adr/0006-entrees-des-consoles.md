# ADR 0006 — Entrées des consoles et hôte USB

- Date : 2026-10-13
- Statut : accepté (plan SP1b-3d)
- Amende : ADR 0004, point 1 (les entrées d'une exécution)

## Contexte

Jusqu'à SP1b-3c, les consoles de l'appareil n'allaient que vers l'hôte. Un firmware qui lit sa console (un invite, un menu, la console de CrossPoint) attendait pour toujours : l'UART n'avait pas de réception, l'USB-Serial-JTAG pas d'hôte. Or l'hôte USB fait partie de ce que voit le firmware : sans trames de début (SOF), le moniteur de connexion d'ESP-IDF et le `HWCDC` d'Arduino jugent qu'aucun hôte n'est là et jettent leur sortie (rapport `docs/research/usb-serial-jtag-host.md`, §B.3 et §B.5).
L'ADR 0004 a dit ce qui entre dans une exécution et annoncé que les octets reçus sur une console s'y ajouteraient, datés en temps d'appareil. Ce qu'un hôte envoie dépend souvent de ce que l'appareil a dit (attendre l'invite avant de taper) : une date fixée d'avance ne suffit pas, et une date lue sur l'horloge de l'hôte ferait dépendre l'exécution de la vitesse de la machine qui la fait tourner.

## Décision

1. **Deux entrées s'ajoutent aux quatre de l'ADR 0004.**
   - **L'hôte USB** de l'USB-Serial-JTAG : `absent` (pas de câble), `attached` (un câble, le port fermé : des trames, rien n'est lu) ou `open` (un programme lit et écrit le port), à la mise sous tension (`--usb-host`, `open` par défaut), puis ses changements datés.
   - **Les octets envoyés** à une console (`uart0`, `uart1`, `usb-serial-jtag`), par morceaux datés.

   Les deux entrent dans la machine par des méthodes d'`Esp32C3` (`send_console`, `set_usb_host`), qui les rangent dans les périphériques comme des événements datés : le modèle ne lit rien sur l'hôte. Une date antérieure au temps d'appareil est refusée par une erreur typée qui donne les deux instants ; rien n'est alors rangé. Une console qui garde déjà un mébioctet en attente refuse le morceau suivant, et la machine garde au plus 16 384 changements d'hôte à venir : une interface qui fournit des entrées sans fin reçoit une erreur et non une mémoire qui grossit.
2. **La date d'une entrée est fixée par les entrées seules, jamais par la façon dont l'exécution est découpée.** Un script de console (`--console-script`) peut dater une étape de l'instant où un texte a atteint l'hôte (`wait-for`). Pour que cet instant soit exact, les canaux qu'un script attend sont « observés » (`watch_console`) : chaque octet qui atteint l'hôte sur un canal observé arrête l'appel de la machine à la frontière d'instruction où il est parti (à son instant même quand le CPU dort), et le temps d'appareil est alors celui de l'octet. Les étapes suivantes sont datées de cet instant et remises toutes à la machine avant qu'elle reprenne. Un test exécute le même script en tranches de 1, de 7 919 et d'un million de cycles, et compare les consoles, l'empreinte de trace et le temps d'appareil.
3. **Le terminal n'est pas une entrée.** Taper au clavier n'a de sens que si le temps d'appareil suit l'horloge murale, ce qu'inkemu ne fait pas : la commande ne relaie pas l'entrée standard. L'interface graphique (SP2) fournira ses octets par la même méthode, datés de l'instant présent de la machine, et dira qu'une telle exécution ne se rejoue qu'avec son enregistrement (SP1b-4).
4. **La console relayée reste une sortie.** `--console` choisit ce qui est montré, pas ce qui est reçu ni exécuté (ADR 0004, point 1).
5. **Un script dit jusqu'où il est allé.** Le résumé de l'exécution imprime l'état de l'hôte USB à la fin et la progression du script ; une exécution qui sortirait avec 0 sort avec 5 quand son script n'est pas allé au bout (un texte encore attendu, une étape datée après la fin). Les autres codes sont gardés : ils en disent plus sur l'exécution.

## Conséquences

- **Enregistrement et rejeu (SP1b-4)** : un enregistrement porte, en plus des entrées de l'ADR 0004, l'état de l'hôte USB à la mise sous tension et la liste datée des entrées des consoles et des changements d'hôte, telle que la machine les a reçues (pas le script, dont les dates dépendent de l'exécution). Un rejeu les remet à la machine d'un coup, au départ.
- **Le script n'est pas le format des scénarios.** Sa première ligne, `inkemu-console-script 1`, le désigne ; les scénarios YAML du sous-projet 4 (boutons, écran, consoles) pourront le remplacer sans ambiguïté.
- **L'hôte d'une UART envoie au débit et dans la trame de l'UART.** Un hôte réel peut se tromper de débit ; ce cas (octets corrompus, erreurs de trame) n'est pas modélisé, et l'hypothèse est écrite dans le code.
- **Les empreintes de trace des exécutions qui écrivent sur l'USB-Serial-JTAG changent.** La ROM recopie sa console sur l'USB-Serial-JTAG ; avant SP1b-3d, l'extrémité IN était toujours libre, elle attend désormais que l'hôte prenne le paquet. Les empreintes épinglées par les tests ont été remesurées avec ce changement (note de passation de SP1b-3d).
- **Numérotation** : l'ADR suivant prend le numéro 0007.
