# ADR-0005 — Le M5 comme cible, le Heltec V4 en C++ comme référence

- Statut : accepté
- Date : 2026-11-05

## Contexte

Tanguy a deux appareils : un ThinkNode M5 (la cible, son appareil Bluetooth) et un Heltec WiFi LoRa 32 V4 déjà flashé en MeshCore C++ v1.18 `companion_radio_usb`, piloté depuis le Mac en USB natif.

## Décision

- Le **M5** reçoit notre firmware dès MC4 ; il n'est plus l'appareil du quotidien pendant le développement. Le retour au firmware C++ se fait en flashant la release officielle (`ThinkNode_M5_companion_radio_ble-v1.18.0-*-merged.bin`), et l'état d'origine est retrouvé (ADR-0006).
- Le **Heltec** reste en C++ et sert de **référence** :
  - pour le protocole compagnon (`mc conform`) ;
  - sur l'air : `LOG_RX_DATA` (0x88) donne chaque trame reçue, `SEND_RAW_PACKET` (65) émet une trame arbitraire.
- Pas de sauvegarde de flash avant de flasher le M5 (choix de Tanguy).

## Conséquences

- Le build Heltec de notre firmware est remis à plus tard ; ses broches (recopiées de loranet) restent dans `board/heltec_v4.rs`.
- Les tests sur l'air se font entre nos deux appareils ; les captures du vrai réseau ne servent qu'en local.
