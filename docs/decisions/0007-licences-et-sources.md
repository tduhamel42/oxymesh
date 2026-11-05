# ADR-0007 — Licences et sources

- Statut : accepté
- Date : 2026-11-05

## Contexte

Le firmware réimplémente MeshCore et s'appuie sur ce que d'autres projets savent du matériel du M5.

## Décision

- **MeshCore** (MIT) et **RadioLib** (MIT) : on réimplémente d'après leurs sources, avec attribution dans `NOTICE`. On ne recopie pas de fichiers.
- **Meshtastic** (GPL-3.0) et **GxEPD2** (GPL-3.0) : **aucun code repris**. On n'en retient que des faits matériels (brochages, adresses I2C, courbe de tension de la batterie, révisions du GPS). Le pilote SSD1681 est écrit d'après la datasheet.
- **loranet** (même auteur) : code recopié librement, origine indiquée dans le message de commit.
- La licence d'oxymesh n'est pas encore choisie (Tanguy décidera).
- Aucune contribution publique (PR à MeshCore, trouble-host, etc.) sans l'accord de Tanguy.

## Conséquences

- Le correctif de trouble-host pour le PIN fixe vit dans `firmware/esp32s3/vendor/` tant que Tanguy n'a pas décidé d'une PR.
