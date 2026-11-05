# ADR-0006 — Table de partitions : le SPIFFS d'origine n'est jamais écrit

- Statut : accepté
- Date : 2026-11-05

## Contexte

Le firmware C++ garde l'identité, les réglages, les contacts, les canaux et les annonces dans un SPIFFS (table arduino-esp32 par défaut pour 4 Mo : `spiffs` en 0x290000, 0x160000 octets ; à confirmer sur l'appareil avant le premier flash). Tanguy veut garder son identité MeshCore, et pouvoir revenir au C++.

## Décision

| Partition | Adresse | Taille | Rôle |
|---|---|---|---|
| nvs | 0x9000 | 0x6000 | appairages Bluetooth |
| phy_init | 0xF000 | 0x1000 | |
| factory | 0x10000 | 0x1C0000 (1,75 Mo) | notre firmware |
| mcdata | 0x1D0000 | 0xC0000 (768 Ko) | sequential-storage : identité, réglages, contacts, canaux, annonces, file d'attente, historique |
| spiffs | 0x290000 | 0x160000 | **SPIFFS du C++, lu seulement** |

- Au premier démarrage (mcdata vide), le firmware lit le SPIFFS avec `mc-spiffs` et importe `/identity/_main.id`, `/prefs.json`, `/contacts3`, `/channels2` et `/bl/*`, puis affiche un résumé.
- Il n'écrit jamais dans 0x290000–0x3EFFFF.
- Repli si le SPIFFS est illisible : transfert par le protocole (`EXPORT/IMPORT_PRIVATE_KEY`, `GET_CONTACTS`, `GET_CHANNEL`).

## Conséquences

- Retour au C++ : flasher la release officielle remet sa table de partitions et son application ; le SPIFFS intact rend l'état d'avant.
- Notre application doit tenir dans 1,75 Mo (vérifié à chaque build).
