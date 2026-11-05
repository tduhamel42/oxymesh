# ADR-0004 — Rôle compagnon d'abord

- Statut : accepté
- Date : 2026-11-05

## Contexte

MeshCore a six rôles (compagnon BLE/USB/WiFi, relais, serveur de salon, capteur, terminal, modem KISS). Le M5 de Tanguy tourne en `companion_radio_ble`.

## Décision

- Ce plan ne livre que le rôle **compagnon**, en Bluetooth et en USB à la fois (comme MultiSerialInterface : chaque trame part sur les deux).
- La couche mesh est écrite pour pouvoir relayer (crochet `allow_forward`, limites de sauts, portée par région), mais le relais reste désactivé, comme dans le compagnon C++ (`repeat.disable_fwd = 1`, et l'option `repeat` n'est permise que sur 433,000 / 869,495 / 918,000 MHz).
- Pas de WiFi.

## Conséquences

- Les rôles relais et salon viendront ensuite sur la même machine, sans refonte.
- Les fonctions côté client vers les relais et salons (connexion, état, télémétrie, requêtes binaires et anonymes, trace, découverte) font partie du compagnon et sont livrées.
