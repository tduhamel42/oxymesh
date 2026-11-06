# ADR-0003 — IO-free core, no allocation, fixed capacities

- Status: accepted
- Date: 2026-11-05

## Context

The same protocol must run on the ESP32-S3 without PSRAM (≈ 320 KB of usable RAM, of which ≈ 72 KB go to Bluetooth), in a multi-node simulator, in a virtual device on the Mac and under fuzzing. loranet validated this approach (its ADR-0001): a pure state machine, driven by runners.

## Decision

- `mc-node` is a pure state machine: inputs `Rx`, `TxDone`, `CadDone`, `Timer`, `AppFrame`, UI events; outputs `RadioCommand` (Listen, Transmit, Cad, Sleep), frames to the app, `StoreOp` (persistence), UI events; `next_wakeup()`.
- No allocation, no floats, no system clock: time (monotonic in ns, and Unix epoch) and randomness are injected.
- Fixed capacities, those of v1.18's M5 build: 350 contacts, 40 channels, 8 expected ACKs, 160 dedup hashes, 16 advert paths. The offline message queue (256 frames) lives in flash, with an index in RAM.
- Persistence goes out as `StoreOp`; at boot, the runner reloads the state through a `Snapshot`.

## Consequences

- The same tests run on the host and describe the device's behavior; `mc emu` and `mc-sim` link several state machines without hardware.
- Estimated RAM budget: contacts 350 × (152 + 32 of cached shared secret) ≈ 64 KB, packets 8 KB, screen 5 KB. The real peak is measured from MC4 on.
- Rejected: porting the C++ as is behind an FFI (we would lose testability and the benefit of Rust).
