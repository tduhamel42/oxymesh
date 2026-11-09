# oxymesh

[![ci](https://github.com/tduhamel42/oxymesh/actions/workflows/ci.yml/badge.svg)](https://github.com/tduhamel42/oxymesh/actions/workflows/ci.yml)

**MeshCore-compatible** firmware written in Rust, first for the Elecrow **ThinkNode M5**
(ESP32-S3, SX1262, 200×200 e-paper screen).

- **Over the air**: interoperates with original MeshCore companions, repeaters and room servers (v1.18).
- **With the app**: the official MeshCore app, meshcore.js and other companion-protocol clients
  work unchanged (Bluetooth and USB).
- **On the device**: a new interface, usable without a phone.

**Status: experimental.**
- It runs on a ThinkNode M5 (board v1.0) and has been checked against MeshCore C++ v1.18, over the air and with the app ([`docs/measurements.md`](docs/measurements.md)).
- The 24-hour endurance run on a live network is still to do.
- Only the companion role and only the M5 are supported.

## Getting started

### Try it without hardware

`mc ui` runs the device's interface in a terminal, on a simulated mesh:

```sh
cargo run --release -p mc-cli -- ui
```

On Linux, `mc` needs the D-Bus headers for its Bluetooth client (`libdbus-1-dev` and `pkg-config` on Debian and Ubuntu).

### Flash a ThinkNode M5

1. **Install the toolchains.**
   - [rustup](https://rustup.rs). The host crates pin their own Rust version in `rust-toolchain.toml`.
   - The Xtensa toolchain: `cargo install espup --locked && espup install --targets esp32s3`.
   - The flasher: `cargo install espflash --locked`.
2. **Plug the M5 in over USB-C.** It shows up as a CH340 serial port.
3. **Build and flash:**

   ```sh
   cd firmware/esp32s3
   . ~/export-esp.sh
   cargo run --release --bin mc-m5
   ```

   The runner flashes with oxymesh's partition table ([`partitions.csv`](firmware/esp32s3/partitions.csv), ADR-0006).

**Moving from the C++ firmware.**
- A fresh M5 imports the C++ identity, settings, contacts and channels at first boot.
- Settings → System → Import from C++ does it again later.

**Going back to the C++ firmware.**
- Flash the official MeshCore release for the M5.
- oxymesh never writes the C++ firmware's storage, so its data is still there. This is how it is designed ([ADR-0006](docs/decisions/0006-partitions-and-migration.md)). The full round trip has not been tested yet.

## Documentation

- **Architecture, rules and commands:** [`CLAUDE.md`](CLAUDE.md).
- **Formats:** [`docs/spec/`](docs/spec/).
- **Decisions:** [`docs/decisions/`](docs/decisions/).
- **Measurements:** [`docs/measurements.md`](docs/measurements.md).

## Compared with the MeshCore C++ firmware

The reference is the original MeshCore v1.18 companion firmware on the same M5.

### What stays the same

- **The radio and the app see no difference.** Packets are byte for byte those of MeshCore v1.18, and so are the companion protocol's commands and replies. The few behaviour changes below are listed with their reasons in [`docs/spec/companion.md`](docs/spec/companion.md) §9, and none of them is visible to an app.
- **Radio settings, contacts, channels, adverts, telemetry permissions and the Bluetooth PIN** work as in the C++.
- **The hardware** is used as the C++ uses it: the clock chip (PCF8563, in UTC) and the GPS (the L76K of board v1.0). The buzzer plays the same tunes.

### New on the device

- **A real interface on the e-paper, with the two buttons.** The C++ shows a few information pages and the last messages, driven by one button; replying or changing a setting needs the phone. oxymesh does it all on the device:
  - **Messages:** conversations, unread first, and a two-button keyboard with word completion (French and English) and quick replies, including "My position" when the GPS has a fix. A message sent from the device shows `sending…`, `delivered 2.4 s` or `failed`.
  - **Contacts and channels:** ping, trace, favourite, share as a QR code, remove; add a `#channel` by typing its name.
  - **Tools:**
    - heard nodes;
    - discovery of nearby repeaters;
    - trace route;
    - ping and range test;
    - signal and noise floor;
    - packet log;
    - airtime and duty cycle;
    - repeater admin (login, status, neighbours);
    - GPS;
    - a map of contacts.
  - **Settings:** the whole tree: radio presets per region, identity, Bluetooth (PIN, paired phones), contacts, display (text size, invert, rotate, light timeout), notifications, telemetry, power, time.
  - **First boot:** a setup flow (region, name, time).
  - **Status:** battery percentage and a charging mark, backlight auto-off, and a big clock on Home.
- **Muted channels.** Not in MeshCore at all. A muted channel keeps its own unread count but never beeps, never blinks the LED and stays out of the header and Home counts.
- **Notifications.**
  - Buzzer off, quiet (same tunes, lower volume) or on, silent while a phone is connected.
  - The blue LED blinks while something is unread (the C++ uses it for transmitting).
- **Messages survive a reboot.**
  - The 256 messages waiting for the app are kept in flash; the C++ keeps them in RAM and loses them when it restarts.
  - The device also keeps its own history of the last 200 messages, with unread state shared with the app: what you read on the screen or the app counts as read on both.
- **Retries for messages written on the device.** Three direct tries, then a reset of the route and a flood, as the app does for its own messages.
- **USB and Bluetooth at the same time.** The C++ M5 build has Bluetooth only. A cable on the USB port works as a second app link.
- **A new Bluetooth PIN applies at once.** The C++ waits for a reboot.
- **Hibernation wakes up on the top button.** The C++ hibernates with no way to wake but a reset.
- **Moving from the C++ is one hold.**
  - Settings → System → Import from C++ brings over its identity, settings, contacts and channels. A fresh M5 imports them at first boot.
  - The C++ storage is only ever read, so flashing the C++ back finds everything as it was.
- **Tools on the computer.** `mc` (in `crates/mc-cli`):
  - `mc ui`: the device's interface in a terminal, on a simulated mesh;
  - `mc duo`, `mc airtest`, `mc burst`: radio tests against a C++ node;
  - `mc spiffs`: what the C++ left on a flash image;
  - `mc var`: read or set the device's custom variables;
  - a Bluetooth client (`ble:<name>` ports).

### Not there yet

- **Only the companion role.** There are no repeater or room-server builds, which the C++ has.
- **The `HAS_CONNECTION` keep-alive table** is not implemented.
- **Only the ThinkNode M5.** The C++ supports dozens of boards. The M5 board v2.0 (AG3352 GPS) is detected but its GPS is not driven, by either firmware.

## Does Rust make it faster?

**Not in a way you would notice. On this device the radio sets the pace, not the processor.** At the usual settings (SF8, 62.5 kHz) one message spends 160 to 440 ms in the air, and the processor's share is a few milliseconds.

Where both firmwares were measured side by side:

| Measurement | oxymesh | MeshCore C++ |
|---|---|---|
| USB command round trip (min / median / max) | 10 / 12 / 37 ms, on the M5 while receiving and redrawing | 10 / 12 / 17 ms, on the Heltec, idle |
| Delivery acknowledgement round trip, same M5, same settings (`mc airtest`, one run each) | 692 ms | 824 ms |
| Ping round trip between an oxymesh M5 and a C++ Heltec (`mc duo`, median of 10, each direction) | 1040 ms from the M5 | 1015 ms from the Heltec |

The USB link is equally quick on both. Ping times differ by less than the run-to-run noise. The single faster acknowledgement is more likely scheduling than raw speed.

What Rust does change is **robustness**:
- **Every decoder that reads bytes from the air or from an app is fuzzed.** 740 million runs found no crash. One command the C++ mishandles: a short `SEND_CHANNEL_TXT_MSG` passes a negative length to `memcpy`, a likely crash. oxymesh refuses it with an error.
- **The protocol core** has no `unsafe` code, no heap allocation and no floating point, with fixed table sizes.
- **Time and randomness are passed in.** That lets whole meshes run in a simulator on a laptop.
- **The same interface code** runs on the e-paper, in a terminal, and in snapshot tests of every screen.

The cost is size. The image is 1.31 MB of the 1.75 MB app partition; Bluetooth is about 300 KB of it, the new interface much of the rest. The C++ image size has not been measured here.

## Radio use

You are responsible for using the radio legally where you are.
- The EU 868 MHz limits (sub-bands, power, duty cycle) are in [`config/eu868.toml`](config/eu868.toml), with their sources. The firmware enforces the duty cycle.
- Region presets are in [`config/regions.toml`](config/regions.toml).

## Contributing

Issues and pull requests are welcome.
- **The rules:** [`CLAUDE.md`](CLAUDE.md) holds the architecture, the code rules and the commands. It also serves as the brief for AI coding agents.
- **Specs come first.** A behaviour change starts in `docs/spec/`.
- **Before sending a change,** run `scripts/ci.sh`.

To report a security problem, see [`SECURITY.md`](SECURITY.md).

## License

MIT ([`LICENSE`](LICENSE)). Attributions and third-party code: [`NOTICE`](NOTICE).

"MeshCore" is the name of the original project. oxymesh is not part of it and is not endorsed by it.
