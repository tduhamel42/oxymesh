# ADR-0006 — Partition table: the original SPIFFS is never written

- Status: accepted
- Date: 2026-11-05

## Context

The C++ firmware keeps the identity, settings, contacts, channels and adverts in a SPIFFS (default arduino-esp32 table for 4 MB: `spiffs` at 0x290000, 0x160000 bytes; to be confirmed on the device before the first flash). A user moving to oxymesh must keep their MeshCore identity, and be able to go back to the C++.

## Decision

| Partition | Address | Size | Role |
|---|---|---|---|
| nvs | 0x9000 | 0x6000 | Bluetooth pairings |
| phy_init | 0xF000 | 0x1000 | |
| factory | 0x10000 | 0x1C0000 (1.75 MB) | our firmware |
| mcdata | 0x1D0000 | 0xC0000 (768 KB) | sequential-storage: identity, settings, contacts, channels, adverts, offline queue, history |
| spiffs | 0x290000 | 0x160000 | **C++ SPIFFS, read only** |

- On first boot (mcdata empty), the firmware reads the SPIFFS with `mc-spiffs` and imports `/identity/_main.id`, `/prefs.json`, `/contacts3`, `/channels2` and `/bl/*`, then shows a summary.
- It never writes to 0x290000–0x3EFFFF.
- Fallback if the SPIFFS is unreadable: transfer through the protocol (`EXPORT/IMPORT_PRIVATE_KEY`, `GET_CONTACTS`, `GET_CHANNEL`).

## Consequences

- Going back to C++: flashing the official release restores its partition table and its application; the untouched SPIFFS brings back the previous state.
- Our application must fit in 1.75 MB (checked on every build).
