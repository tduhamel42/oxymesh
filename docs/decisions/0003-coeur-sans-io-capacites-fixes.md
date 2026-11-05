# ADR-0003 — Cœur sans IO, sans allocation, à capacités fixes

- Statut : accepté
- Date : 2026-11-05

## Contexte

Le même protocole doit tourner sur l'ESP32-S3 sans PSRAM (≈ 320 Ko de RAM utile, dont ≈ 72 Ko pour le Bluetooth), dans un simulateur à plusieurs nœuds, dans un appareil virtuel sur le Mac et sous fuzz. loranet a validé cette approche (son ADR-0001) : une machine à états pure, pilotée par des runners.

## Décision

- `mc-node` est une machine pure : entrées `Rx`, `TxDone`, `CadDone`, `Timer`, `AppFrame`, événements d'interface ; sorties `RadioCommand` (Listen, Transmit, Cad, Sleep), trames vers l'app, `StoreOp` (persistance), événements d'interface ; `next_wakeup()`.
- Pas d'allocation, pas de flottants, pas d'horloge système : l'heure (monotone en ns, et époque Unix) et l'aléa sont injectés.
- Capacités fixes, celles du build M5 de v1.18 : 350 contacts, 40 canaux, 8 accusés attendus, 160 hashes anti-doublon, 16 chemins d'annonce. La file des messages en attente (256 trames) vit en flash, avec un index en RAM.
- La persistance sort sous forme de `StoreOp` ; au démarrage, le runner recharge l'état par un `Snapshot`.

## Conséquences

- Les mêmes tests tournent sur l'hôte et décrivent le comportement de l'appareil ; `mc emu` et `mc-sim` relient plusieurs machines sans matériel.
- Budget RAM estimé : contacts 350 × (152 + 32 de secret partagé en cache) ≈ 64 Ko, paquets 8 Ko, écran 5 Ko. Le pic réel est mesuré dès MC4.
- Écarté : porter le C++ tel quel derrière un FFI (on perd la testabilité et le gain de Rust).
