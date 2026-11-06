# CLAUDE.md — oxymesh

**MeshCore-compatible** firmware written in Rust. First target: the Elecrow **ThinkNode M5** (ESP32-S3-WROOM-1-N4, SX1262, SSD1681 200×200 e-paper, two buttons, brightness potentiometer, GPS, buzzer). Role: **companion** (Bluetooth and USB), like MeshCore's `companion_radio_ble`.

Original plan: `~/.claude/plans/check-meshcore-firmware-especially-sparkling-pelican.md` (2026-11-05). Passages marked **(ADR-xxxx)** refer to `docs/decisions/`.

---

## 1. Non-negotiable constraints

- **Strict, byte-for-byte compatibility with MeshCore v1.18** (ADR-0001):
  - over the air, with the original companions, repeaters and room servers;
  - with the companion protocol (official app, meshcore.js, meshcore-cli, loranet's web app).
  - Only the **deliberate deviations** listed in `docs/spec/companion.md` are allowed, and they must be invisible to the app.
- **Reference**: MeshCore `companion-v1.18.0` (commit 81dbae5). Every behavioral rule in the spec cites the original file and line. **When in doubt, the C++ code is authoritative, not the MeshCore docs.**
- **The original SPIFFS is never written** (ADR-0006): migration on first boot, and lossless return to the C++ firmware.
- **Licenses** (ADR-0007): MeshCore is MIT (attribution in `NOTICE`). Meshtastic and GxEPD2 are GPL: we copy **no code** from them, only hardware facts.
- **EU regulations**: values live in `config/eu868.toml`, never hard-coded. The duty cycle is enforced by the firmware.

## 2. Architecture

### IO-free core (ADR-0003)

The whole protocol is a **pure state machine**: no IO, no system clock, no async, no allocation, fixed capacities (350 contacts, 40 channels). Randomness and time are injected. The firmware, the simulator and the virtual device are just *runners*:

```rust
// Sketch — the exact contract lives in mc-core
fn handle(&mut self, input: Input<'_>, now: Instant, rng: &mut impl RngCore); // Rx | TxDone | CadDone | Timer | AppFrame | Button…
fn poll_command(&mut self) -> Option<RadioCommand>;   // Listen | Transmit | Cad | Sleep
fn poll_output(&mut self) -> Option<Output<'_>>;      // frame to the app | StoreOp | UI event
fn next_wakeup(&self) -> Option<Instant>;
```

### Repository

```
crates/
  mc-core/    no_std  Reader/Writer, time (integer ns), airtime, DutyBudget, state machine contract
  mc-air/     no_std  over-the-air packets (docs/spec/air.md): codec, payloads, crypto, hashes, scope, adverts
  mc-proto/   no_std  companion protocol (docs/spec/companion.md), both directions; Strict (app) and Device decoding
  mc-node/    no_std  the state machine: dispatcher, mesh, chat, companion, settings; persistence as StoreOp
  mc-spiffs/  no_std  read-only SPIFFS reader and importers for the C++ firmware
  mc-ui/      no_std  200×200 screens (docs/spec/ui.md) on Ratatui + mousefood, with alloc (ADR-0008); gestures, e-paper refresh policy
  mc-sim/     std     simple simulated radio linking several mc-node instances
  mc-cli/     std     `mc` binary: info, probe, sniff, inject, emu, conform, spiffs, ui
firmware/esp32s3/     SEPARATE workspace (Xtensa `esp` toolchain): boards, drivers, runner
fuzz/                 cargo-fuzz targets (separate workspace, nightly)
config/               regions (radio preset + scope) and regulations (TOML, with sources)
docs/spec/            specifications — source of truth
docs/decisions/       short ADRs (one decision = one file)
testdata/             test vectors (independent Python oracle, meshcore.js, captures from our own devices)
```

Part of the code comes from loranet (`~/Documents/Perso/Dev/loranet`, same author): it is **copied** then adapted, with its origin stated in the commit message. No dependency between the two repositories, and loranet is never modified from here.

## 3. Code rules

- Core crates: `#![no_std]`, `#![forbid(unsafe_code)]` (workspace lints), **no allocation, no floats** (`clippy::float_arithmetic` denied, `scripts/ci.sh` searches for `f32`/`f64`).
- `mc-ui` is the one `no_std` crate that allocates (Ratatui's cell buffers, ADR-0008). On the device: one long-lived `Terminal`, plain `Rect` arithmetic instead of `Layout`, mousefood config built field by field (its `default()` leaks a font atlas).
- **Hardened parsers** (they read bytes from strangers over the air or from an app):
  - zero-copy on `&[u8]`, never panic on external input: clippy `deny(unwrap_used, expect_used, indexing_slicing, panic)` outside tests;
  - over the air: `decode → encode` must give back exactly the same bytes, otherwise reject;
  - app commands, device side: we accept **exactly** what the C++ accepts (optional trailing fields, extra bytes ignored) (ADR-0002).
- Hand-written formats, no serde over the air or in the companion protocol. Serde is allowed for config and tools.
- **Spec first**: every rule starts in `docs/spec/`, then vectors, then code. An ambiguity → an ADR.
- Everything is in **English**: docs, specs, ADRs, comments, identifiers, commit messages and **text shown to the user (screen, device CLI)**.

## 4. Tests

- Unit tests and `proptest` (round trip, canonicity).
- Vectors in `testdata/`, checked by the tests:
  - over the air: **independent Python oracle** (`testdata/air/gen.py`, pure Python with no dependencies: AES, X25519, Ed25519 RFC 8032);
  - companion protocol: meshcore.js (`testdata/proto/gen.mjs`);
  - captures from **our own devices** only. Captures from the real network stay local (`/local/`, ignored by git).
- One `cargo-fuzz` target per decoder, created **together with** it.
- `mc conform`: differential comparison of the C++ firmware's responses (Heltec V4, reference) with ours.
- Hardware: M5 = device under test, Heltec V4 running MeshCore C++ v1.18 USB = reference (`LOG_RX_DATA` and `SEND_RAW_PACKET` make it an over-the-air oracle).

## 5. Roadmap

Each milestone is committed locally once finished and tested. Nothing is pushed without Tanguy's approval.

0. **MC0** — repository, ADRs 0001-0007, `air`, `companion`, `m5`, `ui` specs.
1. **MC1** — `mc-core` + `mc-air`; Python oracle; `mc info|sniff|inject`.
2. **MC2** — complete `mc-proto` (66 commands, 29 responses, 17 notifications), Strict and Device modes.
3. **MC3** — `mc-node` + `mc-sim`; `mc emu`, `mc conform`.
4. **MC4** — the M5 over USB: radio (transmit, CAD), storage, board, battery, clock, GPS, potentiometer.
5. **MC5** — Bluetooth (NUS, LESC, PIN, MTU), official app and loranet's web app.
6. **MC6** — migration from the C++ firmware's SPIFFS.
7. **MC7** — new e-paper interface.
8. **MC8** — 24 h endurance run on the network, and measurements (`docs/measurements.md`).

Later: repeater and room server roles, Heltec V4, nRF52 (ThinkNode M1, T-Echo), light sleep.

## 6. Do not

- No change to the over-the-air format or the companion protocol: we are compatible or we are nothing.
- No writes to the C++ firmware's SPIFFS.
- No code copied from Meshtastic or GxEPD2.
- No hard-coded regulatory values.
- No captures from the real network (names, other people's messages) in the repository.
- No mandatory flash backup before flashing (Tanguy does not want one).
- No public PR (MeshCore, trouble-host…) without Tanguy's approval.

## 7. Commands

```sh
scripts/ci.sh                         # everything CI does (CI_SKIP_FUZZ=1, CI_SKIP_FIRMWARE=1 to skip)
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
scripts/fuzz.sh N [target…]           # fuzz, N seconds per target
python3 -I testdata/air/gen.py > testdata/air/vectors.toml   # over-the-air vectors (pure Python oracle)
(cd testdata/proto && npm ci && node gen.mjs vectors.toml)   # companion vectors, cross-checked with meshcore.js
cargo run --release -p mc-cli -- info /dev/cu.wchusbserial120          # query a USB companion
cargo run --release -p mc-cli -- sniff /dev/cu.usbmodem141101 --channel '#test'   # packets heard, decoded
cargo run --release -p mc-cli -- airtest --ref /dev/cu.usbmodem141101 --dut /dev/cu.wchusbserial120   # mc-air against a C++ node, zero-hop
cargo run --release -p mc-cli -- ui                  # the device interface in a terminal, on a simulated device (--compact)
cargo run --release -p mc-cli -- ui --png local/ui-png   # every screen of the tour as panel-exact PNG
INSTA_UPDATE=always cargo test -p mc-ui              # accept changed screen snapshots (review the diff!)
cargo run --release -p mc-cli -- advert <port> [--flood]   # make a companion advertise (also sets its clock)
cargo run --release -p mc-cli -- send <port> <key prefix hex> "text"   # direct message, waits for the delivery
cargo run --release -p mc-cli -- inbox <port>               # fetch and print the messages waiting on a companion
cargo run --release -p mc-cli -- burst --a <receiver> --b <sender> --count 20 --gap 1500   # B sends A a burst of DMs; confirmations and what A queued
cargo run --release -p mc-cli -- latency <port> --count 150   # USB command round trips (a stalled main loop shows here)
cargo run --release -p mc-cli -- duo --a /dev/cu.wchusbserial120 --b /dev/cu.usbmodem141101   # two companions talk (adverts, DMs, channel, pings), zero-hop; --flood for more
```

The Heltec also runs loranet's echo bot (`ln mc bot`, started from `../loranet`): ask Tanguy before stopping it, then restart it afterwards with the same command and the same `--state`.

Devices on Tanguy's Mac: Heltec V4 (C++ reference) on `/dev/cu.usbmodem*`, ThinkNode M5 on `/dev/cu.wchusbserial*` (CH340 bridge, WCH driver installed). The numbers change whenever USB re-enumerates: check `ls /dev/cu.*` first.
