# ADR-0009 — One runner, thin boards

- Status: accepted
- Date: 2026-11-09

## Context

oxymesh ran on one board, and `bin/mc-m5.rs` held everything: the SX1262 loop (receive, listen before talk, latched IRQs, noise floor, AGC reset), the app link (USB framing and Bluetooth), the boot steps (partitions, identity, import from the C++ firmware) and the Bluetooth host, next to the M5's pins, e-paper, buttons and expander. A second board (the Heltec V4: same ESP32-S3 and SX1262, an amplifier in front of the antenna, an OLED, one button, the chip's own USB) would have copied most of it.

## Decision

- `firmware/esp32s3/src/run/` holds what every board shares:
  - `radio::Radio`: the radio side of the main loop, with the board's front end behind `FrontEnd` (switched to transmit before a frame and back to receive after it, like the C++'s `onBeforeTransmit` / `onAfterTransmit`) and the chip wiring in `Chip` (TCXO, regulator, current limit, the Heltec's RX register patch);
  - `link::AppLink`: companion frames on a serial `Wire` (UART0 behind the M5's CH340, USB-Serial-JTAG on the Heltec) and over Bluetooth;
  - `boot`: partitions, store, identity (stored, imported, or new), Bluetooth name and PIN;
  - `bt::serve`: the Bluetooth host joined with the board's main loop.
- A board binary (`bin/mc-m5.rs`, `bin/mc-heltec-v4.rs`) keeps its pins, peripherals, screen, buttons and power handling, and calls the shared parts each pass.
- What the app sees of a board is in `mc_node::device::Info` (manufacturer, maximum TX power, GPS, front end).

## Consequences

- The M5's behaviour is unchanged; its loop is about 230 lines shorter.
- The screen loop stays per board: e-paper with a refresh policy and two buttons on the M5, an OLED with status pages and one button on the Heltec.
- Rejected: one generic `main` over a `Board` trait with every peripheral behind it. The boards differ most where the trait would be widest (screen, buttons, power), and esp-hal's peripherals are concrete types taken once in `main`.
