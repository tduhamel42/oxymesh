# CLAUDE.md — oxymesh

Firmware **compatible MeshCore** écrit en Rust. Première cible : l'Elecrow **ThinkNode M5** (ESP32-S3-WROOM-1-N4, SX1262, e-paper SSD1681 200×200, deux boutons, potentiomètre de luminosité, GPS, buzzer). Rôle : **compagnon** (Bluetooth et USB), comme `companion_radio_ble` de MeshCore.

Plan d'origine : `~/.claude/plans/check-meshcore-firmware-especially-sparkling-pelican.md` (2026-11-05). Les passages marqués **(ADR-xxxx)** renvoient à `docs/decisions/`.

---

## 1. Contraintes non négociables

- **Compatibilité stricte, octet pour octet, avec MeshCore v1.18** (ADR-0001) :
  - sur l'air, avec les compagnons, relais et serveurs de salon d'origine ;
  - avec le protocole compagnon (app officielle, meshcore.js, meshcore-cli, app-web de loranet).
  - Seuls les **écarts volontaires** listés dans `docs/spec/companion.md` sont permis, et ils doivent être invisibles pour l'app.
- **Référence** : MeshCore `companion-v1.18.0` (commit 81dbae5). Toute règle de comportement de la spec cite le fichier et la ligne d'origine. **En cas de doute, le code C++ fait foi, pas la doc de MeshCore.**
- **Le SPIFFS d'origine n'est jamais écrit** (ADR-0006) : migration au premier démarrage et retour au firmware C++ sans perte.
- **Licences** (ADR-0007) : MeshCore est sous MIT (attribution dans `NOTICE`). Meshtastic et GxEPD2 sont sous GPL : on n'en recopie **aucun code**, seulement des faits matériels.
- **Réglementation UE** : valeurs dans `config/eu868.toml`, jamais en dur. Le duty cycle est borné par le firmware.

## 2. Architecture

### Cœur sans IO (ADR-0003)

Tout le protocole est une **machine à états pure** : sans IO, sans horloge système, sans async, sans allocation, à capacités fixes (350 contacts, 40 canaux). L'aléa et l'heure sont injectés. Le firmware, le simulateur et l'appareil virtuel ne sont que des *runners* :

```rust
// Esquisse — le contrat exact est dans mc-core
fn handle(&mut self, input: Input<'_>, now: Instant, rng: &mut impl RngCore); // Rx | TxDone | CadDone | Timer | AppFrame | Button…
fn poll_command(&mut self) -> Option<RadioCommand>;   // Listen | Transmit | Cad | Sleep
fn poll_output(&mut self) -> Option<Output<'_>>;      // trame vers l'app | StoreOp | événement d'interface
fn next_wakeup(&self) -> Option<Instant>;
```

### Dépôt

```
crates/
  mc-core/    no_std  Reader/Writer, temps (ns entiers), temps d'antenne, DutyBudget, contrat de la machine
  mc-air/     no_std  trames sur l'air (docs/spec/air.md) : codec, charges utiles, crypto, hashes, portée, annonces
  mc-proto/   no_std  protocole compagnon (docs/spec/companion.md), dans les deux sens ; décodage Strict (app) et Device
  mc-node/    no_std  la machine : dispatcher, mesh, chat, compagnon, réglages ; persistance en StoreOp
  mc-spiffs/  no_std  lecteur SPIFFS en lecture seule et importeurs du firmware C++
  mc-ui/      no_std  écrans 200×200 (docs/spec/ui.md) sur embedded-graphics, politique de rafraîchissement e-paper
  mc-sim/     std     radio simulée simple pour relier plusieurs mc-node
  mc-cli/     std     binaire `mc` : info, probe, sniff, inject, emu, conform, spiffs, ui
firmware/esp32s3/     workspace SÉPARÉ (toolchain Xtensa `esp`) : cartes, pilotes, runner
fuzz/                 cibles cargo-fuzz (workspace séparé, nightly)
config/               régions (preset radio + portée) et réglementation (TOML, avec sources)
docs/spec/            spécifications — source de vérité
docs/decisions/       ADR courts (une décision = un fichier)
testdata/             vecteurs de test (oracle Python indépendant, meshcore.js, captures de nos appareils)
```

Une partie du code vient de loranet (`~/Documents/Perso/Dev/loranet`, même auteur) : il est **recopié** puis adapté, avec l'origine dans le message de commit. Aucune dépendance entre les deux dépôts, et loranet n'est jamais modifié depuis ici.

## 3. Règles de code

- Crates du cœur : `#![no_std]`, `#![forbid(unsafe_code)]` (lints du workspace), **pas d'allocation, pas de flottants** (`clippy::float_arithmetic` refusé, `scripts/ci.sh` cherche `f32`/`f64`).
- **Parseurs durcis** (ils lisent des octets venus d'inconnus sur l'air ou d'une app) :
  - zéro-copie sur `&[u8]`, jamais de panic sur une entrée externe : clippy `deny(unwrap_used, expect_used, indexing_slicing, panic)` hors tests ;
  - sur l'air : `decode → encode` redonne exactement les mêmes octets, sinon rejet ;
  - commandes de l'app, côté appareil : on accepte **exactement** ce qu'accepte le C++ (champs finaux optionnels, octets en trop ignorés) (ADR-0002).
- Formats écrits à la main, pas de serde sur l'air ni dans le protocole compagnon. Serde permis pour la config et les outils.
- **Spec d'abord** : toute règle commence dans `docs/spec/`, puis vecteurs, puis code. Une ambiguïté → un ADR.
- Docs, specs, ADR et commentaires en **français** ; identifiants en anglais ; **textes affichés à l'utilisateur (écran, CLI de l'appareil) en anglais**.

## 4. Tests

- Tests unitaires et `proptest` (aller-retour, canonicité).
- Vecteurs dans `testdata/`, vérifiés par les tests :
  - sur l'air : **oracle Python indépendant** (`testdata/air/gen.py`, Python pur sans dépendance : AES, X25519, Ed25519 RFC 8032) ;
  - protocole compagnon : meshcore.js (`testdata/proto/gen.mjs`) ;
  - captures de **nos propres appareils** seulement. Les captures du vrai réseau restent locales (`/local/`, ignoré par git).
- Une cible `cargo-fuzz` par décodeur, créée **en même temps** que lui.
- `mc conform` : comparaison différentielle des réponses du firmware C++ (Heltec V4, référence) et des nôtres.
- Matériel : M5 = appareil sous test, Heltec V4 en MeshCore C++ v1.18 USB = référence (`LOG_RX_DATA` et `SEND_RAW_PACKET` en font un oracle sur l'air).

## 5. Feuille de route

Chaque étape est commitée localement quand elle est finie et testée. Rien n'est poussé sans l'accord de Tanguy.

0. **MC0** — dépôt, ADR 0001-0007, specs `air`, `companion`, `m5`, `ui`.
1. **MC1** — `mc-core` + `mc-air` ; oracle Python ; `mc info|sniff|inject`.
2. **MC2** — `mc-proto` complet (66 commandes, 29 réponses, 17 notifications), modes Strict et Device.
3. **MC3** — `mc-node` + `mc-sim` ; `mc emu`, `mc conform`.
4. **MC4** — le M5 en USB : radio (émission, CAD), stockage, carte, batterie, horloge, GPS, potentiomètre.
5. **MC5** — Bluetooth (NUS, LESC, PIN, MTU), app officielle et app-web de loranet.
6. **MC6** — migration depuis le SPIFFS du firmware C++.
7. **MC7** — nouvelle interface e-paper.
8. **MC8** — endurance 24 h sur le réseau et mesures (`docs/mesures.md`).

Plus tard : rôles relais et salon, Heltec V4, nRF52 (ThinkNode M1, T-Echo), sommeil léger.

## 6. À ne pas faire

- Pas de changement de format sur l'air ou du protocole compagnon : on est compatible ou on n'est rien.
- Pas d'écriture dans le SPIFFS du firmware C++.
- Pas de code recopié de Meshtastic ni de GxEPD2.
- Pas de valeurs réglementaires en dur.
- Pas de captures du vrai réseau (noms, messages d'autres gens) dans le dépôt.
- Pas de sauvegarde de flash imposée avant un flash (Tanguy n'en veut pas).
- Pas de PR publique (MeshCore, trouble-host…) sans l'accord de Tanguy.

## 7. Commandes

```sh
scripts/ci.sh                         # tout ce que fait la CI (CI_SKIP_FUZZ=1, CI_SKIP_FIRMWARE=1 pour sauter)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/fuzz.sh N [cible…]            # fuzz, N secondes par cible
```

Appareils sur le Mac de Tanguy : Heltec V4 (référence C++) sur `/dev/cu.usbmodem141101`, ThinkNode M5 sur `/dev/cu.wchusbserial120` (pont CH340, pilote WCH installé).
