# ADR-0007 — Licenses and sources

- Status: accepted
- Date: 2026-11-05

## Context

The firmware reimplements MeshCore and relies on what other projects know about the M5's hardware.

## Decision

- **MeshCore** (MIT) and **RadioLib** (MIT): we reimplement from their sources, with attribution in `NOTICE`. We do not copy files.
- **Meshtastic** (GPL-3.0) and **GxEPD2** (GPL-3.0): **no code taken**. We only keep hardware facts (pinouts, I2C addresses, battery voltage curve, GPS revisions). The SSD1681 driver is written from the datasheet.
- **loranet** (same author): code copied freely, origin stated in the commit message.
- oxymesh is released under the MIT license (`LICENSE`).
- No public contribution (PR to MeshCore, trouble-host, etc.) without the maintainer's approval.

## Consequences

- trouble-host's fixed PIN lives in `firmware/esp32s3/vendor/`: trouble-host 0.7.0 with `set_passkey` backported from 0.8.0, which already has it upstream (no PR needed). It goes away once esp-radio supports bt-hci 0.10 (`firmware/esp32s3/vendor/README.md`).
