# ADR-0002 — Decode app commands like the C++, not canonically

- Status: accepted
- Date: 2026-11-05

## Context

The hardened-parser rule (loranet) requires canonical encoding: `decode → encode` must give back the same bytes, otherwise reject. But the C++ firmware accepts commands with optional trailing fields (`SET_RADIO_PARAMS` without the `repeat` byte, `GET_CONTACTS` without `since`…), ignores extra bytes, and answers `ERR 1` (not `ERR 6`) when a length precondition fails. Existing apps rely on these tolerances.

## Decision

- `mc-proto` has two decoding modes:
  - **`Strict`** (app side, tools, tests): canonical, as in loranet;
  - **`Device`** (device side): accepts exactly what v1.18's `MyMesh::handleCmdFrame` accepts, with the same error codes. Each tolerance is listed in `docs/spec/companion.md` with its C++ line.
- **Over-the-air** packets are still decoded canonically: we keep the raw bytes when the C++ tolerates a tail (4- to 6-byte ACK), so that re-encoding is identical.
- The safety rules do not change: zero-copy, never panic, never allocate.

## Consequences

- The `cmd_decode` fuzz target checks, in `Device` mode, that every accepted command re-encodes into a canonical form that decodes in `Strict` mode with the same meaning.
- Rejected: rejecting what the C++ accepts (would break apps); accepting more than the C++ (behavior not testable against the reference).
