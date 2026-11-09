# ADR-0005 — The M5 as target, the Heltec V4 running C++ as reference

- Status: accepted
- Date: 2026-11-05

## Context

The project has two devices: a ThinkNode M5 (the target, the Bluetooth device in daily use) and a Heltec WiFi LoRa 32 V4 already flashed with MeshCore C++ v1.18 `companion_radio_usb`, driven from the Mac over native USB.

## Decision

- The **M5** gets our firmware from MC4 on; it is no longer the everyday device during development. Going back to the C++ firmware is done by flashing the official release (`ThinkNode_M5_companion_radio_ble-v1.18.0-*-merged.bin`), and the original state is recovered (ADR-0006).
- The **Heltec** stays on C++ and serves as the **reference**:
  - for the companion protocol (`mc conform`);
  - over the air: `LOG_RX_DATA` (0x88) reports every received packet, `SEND_RAW_PACKET` (65) transmits an arbitrary packet.

## Consequences

- The Heltec build of our firmware is postponed; its pins (copied from loranet) stay in `board/heltec_v4.rs`.
- Over-the-air tests run between our two devices; captures from the real network are only used locally.
