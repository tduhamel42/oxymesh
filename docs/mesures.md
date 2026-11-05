# Mesures et essais sur matériel

Journal des essais sur les vrais appareils. Chaque entrée : date, montage, commande, résultat.

## 2026-11-05 — `mc airtest` : mc-air contre MeshCore C++ v1.18 (MC1)

- Montage : Heltec V4 en `companion_radio_usb` v1.18.0 (référence, `/dev/cu.usbmodem141101`) ; ThinkNode M5 en `companion_radio_usb` v1.18.0-81dbae5 (appareil sous test, `/dev/cu.wchusbserial120`). Région fr : 869,618 MHz, 62,5 kHz, SF8, CR 4/8.
- Commande : `mc airtest --ref /dev/cu.usbmodem141101 --dut /dev/cu.wchusbserial120`.
- Tout en zéro saut (route directe, chemin vide) : rien n'est relayé sur le réseau.
- Résultat : **10 vérifications sur 10** au premier essai.

| Vérification | Résultat |
|---|---|
| Annonce signée par mc-air (identité de test orlp) acceptée par le C++ | contact ajouté (ADVERT) |
| DM chiffré par mc-air, lu par le C++ | texte identique ; `path_len` 0xFF (direct) |
| Accusé du C++ égal au CRC calculé par mc-air | identique, 6 octets |
| DM du C++, déchiffré par mc-air | texte identique |
| CRC d'accusé calculé par mc-air égal à celui annoncé par le C++ dans `SENT` | identique |
| Accusé construit par mc-air reconnu par le C++ | `SEND_CONFIRMED`, aller-retour 824 ms |
| Annonce du C++ vérifiée par mc-air | signature ok, nom identique |
| Datagramme de canal du C++ (zéro saut) déchiffré par mc-air | identique |
| Message de canal de mc-air lu par le C++ | identique |

Autres constats :
- Le M5 rapporte 869618 kHz dans `SELF_INFO` : l'écart 2 annoncé par l'étude (869617) n'existe pas dans les bandes usuelles (ADR-0001 corrigé).
- `mc sniff` sur le M5 pendant 2 min (22 h 15) : aucune trame entendue, réseau calme à cette heure.
