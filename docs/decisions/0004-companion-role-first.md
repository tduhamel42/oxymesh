# ADR-0004 — Companion role first

- Status: accepted
- Date: 2026-11-05

## Context

MeshCore has six roles (BLE/USB/WiFi companion, repeater, room server, sensor, terminal, KISS modem). Tanguy's M5 runs `companion_radio_ble`.

## Decision

- This plan only delivers the **companion** role, over Bluetooth and USB at the same time (like MultiSerialInterface: each frame goes out on both).
- The mesh layer is written so that it can forward (`allow_forward` hook, hop limits, per-region scope), but forwarding stays disabled, as in the C++ companion (`repeat.disable_fwd = 1`, and the `repeat` option is only allowed on 433.000 / 869.495 / 918.000 MHz).
- No WiFi.

## Consequences

- The repeater and room server roles will come later on the same state machine, without a redesign.
- The client-side features towards repeaters and room servers (login, status, telemetry, binary and anonymous requests, trace, discovery) are part of the companion and are delivered.
