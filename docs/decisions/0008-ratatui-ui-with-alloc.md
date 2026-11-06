# ADR-0008 — On-device interface on Ratatui, with a heap for the UI only

- Status: accepted
- Date: 2026-11-06

## Context

The M5's screen is a 200×200 one-bit e-paper panel with two buttons. Tanguy wants a real interface (messaging, menus, tools, settings) and asked whether Ratatui would fit. Ratatui 0.30 runs `no_std` with `alloc` (`ratatui-core`, `ratatui-widgets`), and `mousefood` 0.5 draws it with embedded-graphics. It needs a heap for its cell buffers. ADR-0003 forbids allocation in the protocol core, and the ESP32-S3 has no PSRAM, while Bluetooth (MC5) will need about 72 KB of heap. The U0 spike measured the cost on the M5 before committing to it (docs/measurements.md, 2026-11-06).

## Decision

- The interface (`mc-ui`) is built on `ratatui-core` + `ratatui-widgets`, drawn by `mousefood` straight into the panel frame (`mc_ui::panel::Frame`): `monochrome` and `fonts` features, no `framebuffer` feature (the frame is the framebuffer).
- `mc-ui` is `no_std` with `alloc`. Only the interface allocates; `mc-core`, `mc-air`, `mc-proto` and `mc-node` stay allocation-free (ADR-0003 unchanged).
- One `Terminal` lives for the whole session. It is rebuilt only when the text size changes.
- Screens split the area with plain `Rect` arithmetic on the fixed grid, never with `Layout` and its constraint solver.
- The backend config is built field by field, never from `EmbeddedBackendConfig::default()`.
- Grids: large text 25×15 (8×13 font), compact 33×20 (6×10 font).
- Budget on the M5: UI heap 40 KB, from the main DRAM (the stack keeps the rest). The 72 KB `dram2` region stays for Bluetooth.
- The same `mc-ui` code runs in the emulator (`mc ui`, crossterm) and in snapshot tests (`TestBackend`).

## Consequences

- Measured on the M5 (U0):

  | | Large 25×15 | Compact 33×20 |
  |---|---|---|
  | Heap, steady | 21.0 KB | 37.0 KB |
  | Heap, peak | 22.6 KB | 39.4 KB |
  | Draw, whole screen | 6 ms | 6 ms |
  | Input to visible (partial refresh) | 549 ms | 549 ms |

  A full refresh takes 2.0 s.
- `Layout` cost about 30 KB of transient heap and 15 ms per draw on the M5. Plain arithmetic removed both.
- With the `fonts` feature, `EmbeddedBackendConfig::default()` builds the default font atlas and leaks it, about 3.8 KB per call. Building the config field by field removed the leak; the host check shows 0 bytes left after a create and drop cycle.
- Text is monospace only. Big clock digits, QR codes and icons are drawn over the Ratatui frame with embedded-graphics (overlays).
- The two missing glyphs ✓ and ✗ (absent from the 8×13 font) are overlays or text.
- Rejected: drawing the screens directly with embedded-graphics, which allocates nothing. We would lose the widgets, the terminal emulator and the snapshot tests, and the measured RAM cost does not justify it.
- Rejected: a 40 KB framebuffer through mousefood's `framebuffer` feature, which would double the frame for nothing.
