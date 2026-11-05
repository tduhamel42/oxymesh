# oxymesh

Firmware **compatible MeshCore** écrit en Rust, d'abord pour l'Elecrow **ThinkNode M5**
(ESP32-S3, SX1262, écran e-paper 200×200).

- **Sur l'air** : interopère avec les compagnons, relais et serveurs de salon MeshCore d'origine (v1.18).
- **Avec l'app** : l'app MeshCore officielle, meshcore.js et les autres clients du protocole
  compagnon marchent sans changement (Bluetooth et USB).
- **Sur l'appareil** : une nouvelle interface, utilisable sans téléphone.

Feuille de route, règles et commandes : [`CLAUDE.md`](CLAUDE.md). Formats : [`docs/spec/`](docs/spec/).
Décisions : [`docs/decisions/`](docs/decisions/). Attributions : [`NOTICE`](NOTICE).
