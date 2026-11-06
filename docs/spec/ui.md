# ThinkNode M5 interface

Source of truth for `mc-ui`. On-screen text **in English**. 200×200 black and white e-paper panel (SSD1681), two buttons, brightness potentiometer, blue LED, buzzer.

## 1. Principles

- **Built on Ratatui** (`ratatui-core` + `ratatui-widgets`), drawn by `mousefood` into the panel frame (ADR-0008). Text is a grid of monospace cells; big digits and QR codes are pixel overlays drawn after the grid.
- **The same code runs everywhere**: on the M5, in the terminal emulator (`mc ui`), and in snapshot tests. Screen logic is separate from drawing, so it is tested without a terminal.
- **Both buttons mean the same thing on every screen**, and the footer always says what they do right now.
- **Meaning never rests on style alone**: the selected row is reversed *and* marked; states are words or symbols, not just bold.
- **The screen never blocks the radio or Bluetooth**: the panel's BUSY line is polled, never waited on.
- **Nothing disappears from the app on its own**: the protocol's offline queue is untouched by the screen (`docs/spec/companion.md` §5).

## 2. Grids and glyphs

| Grid | Font | Cells | Spare pixels |
|---|---|---|---|
| Large (default) | 8×13 (bold 8×13 for titles) | 25 × 15 | 5 rows |
| Compact | 6×10 | 33 × 20 | 2 columns |

- Only glyphs present in the font are used. Each grid has its own symbol set (`theme::Symbols`), and a test checks every symbol against its font. A missing glyph would print as `?`.
- Missing from both fonts: ✓ ✗ ✉ ⚡. Missing from 6×10: ▸ ◂ ⏎ ★. Their fallbacks are words or ASCII.
- Screens split the area with plain arithmetic on the grid, never with Ratatui's `Layout` (ADR-0008).

## 3. Buttons and gestures

| Button | Click | Double click (< 300 ms) | Long press (≥ 600 ms) | Hold (≥ 1 s) |
|---|---|---|---|---|
| **A** (GPIO21, bottom) | next / down | previous / up | open, select, confirm | repeats "next", faster after 2 s and 4 s |
| **B** (GPIO14, top) | back (nothing on Home) | Home from anywhere | backlight on / off | — |

- Recognizer (`mc_ui::input`): 30 ms debounce; a click is reported once the 300 ms double-click window has passed; a long press is reported on release; while held, a tick every 250 ms from 1 s on. A released after it started repeating does not also open.
- Lists wrap: "next" on the last row goes to the first, "previous" on the first goes to the last.
- **Hold-to-confirm** for dangerous or outgoing actions (factory reset, remove contact, flood advert, region change): a popup asks to hold A for 2 s and shows the time left; releasing early cancels.
- The first press while the backlight has timed out only turns it back on (if it was on before).
- For 8 s after boot, a long press on A opens the recovery screen (settings reset, migration restart).

## 4. Screen layout

- **Header** (row 0, reversed): the screen's title on the left (bold); on the right, in order: unread (`●3`), backlight (`☀` while on), Bluetooth (from MC5), time `HH:MM` (not on Home, which has the big clock), battery `87%` (`87%+` while charging).
- **Body**: rows 1 to the last but one.
- **Footer** (last row): the current meaning of the buttons, A on the left, B on the right: `A▸next hold▸open  B◂back` (compact grid: ► ◄). On Home, where B only toggles the light: `B☀`. "hold" is a word: the 8×13 font's ⏎ reads as a music note on the panel.
- **Toasts**: short results ("Sent", "Delivered", "Failed"), shown in place of the footer for 3 s, from the next refresh.
- **Popups**: centered, framed, modal. B click closes them (cancel).
- **New message banner**: over the top of the body, sender and the start of the text, for direct messages (channel messages only count as unread). The first A press opens the conversation and is never swallowed; B dismisses it (`B◂later`). No banner for the conversation already on screen.

## 5. Backlight and potentiometer

- The backlight stays on once enabled, so that its brightness can be adjusted with the potentiometer.
- The potentiometer is not readable by the firmware (`docs/spec/m5.md`): no dead zone is possible at the bottom of its travel. Turning it 100 % off is guaranteed by button B and the auto-off, which cut the LED power (IO5), whatever the knob position. The header always shows the backlight state (Tanguy, 2026-11-06).
- Adjustable auto-off: 30 s, 2 min (default), never; the timer restarts on each press.

## 6. Screens

```
Home (always-on face)
├─ Messages ── conversations (DMs + channels, unread first) ── Conversation ── Compose
├─ Contacts ── list (favourites, recent) ── Contact: info · message · ping · trace · telemetry
│                                          · reset path · favourite · share (QR) · remove
├─ Channels ── list ── channel view ── compose ; add #hashtag channel ; show QR ; remove
├─ Tools
│   ├─ Heard nodes   ├─ Discover    ├─ Trace route   ├─ Ping & range test   ├─ Signal
│   ├─ Packet log    ├─ Airtime     ├─ Repeater admin ├─ GPS               └─ Map
└─ Settings
    ├─ Radio ├─ Identity ├─ Bluetooth ├─ Contacts ├─ Display ├─ Notifications
    ├─ Telemetry ├─ Power ├─ Time └─ System (about, setup again, reboot, factory reset, import)
```

- **Home**: big clock (overlay) and date; the next lines show unread DMs and channels, the last message (sender and start of the text), the radio (preset, last heard), and the GPS fix when enabled. At the bottom, a launcher row (`Messages · Contacts · Channels · Tools · Settings`) moved with A and opened with a long A.
- **Messages**: `+ New message` (pick a contact, then write), then the conversations: unread first, then the most recent; the right column shows the unread count or the time.
- **Conversation**: received on the left, sent on the right, wrapped, newest at the bottom; A scrolls (click down, double up), a long A writes. Sent: `sending…`, `try 2…`, `flood…`, `delivered 2.4 s`, `failed`; channels: `sent`. Received: `direct` or `3 hops`; on channels, the sender's name. Messages arriving while the conversation is open are read at once.
- **Contact**: type and route (`direct`, `2 hops`, `no route yet`), short key, last heard; then Write, Messages, Ping (round trip shown on the screen), Favourite, Reset route, Trace route (U4), Share (QR card), Remove (hold-to-confirm).
- **Channels**: `+ Add #channel` (name typed, secret derived from it like the app), then the channels with their unread count. A channel with unread messages opens on its conversation; otherwise on its screen: Messages, Write, Remove (hold-to-confirm).
- **Compose and quick replies**: §7. 8 quick replies by default: OK, Yes, No, On my way, Call me, Where are you?, Running late, Thanks; plus "Send my position". Sent from the device: 3 direct attempts, then `RESET_PATH` and flood (deliberate deviation 6).
- **Tools**:
  - Heard nodes (name, type, signal bars and SNR, hops, distance, last heard; repeaters first).
  - Discover (zero-hop discovery, SNR both ways, at most once every 5 s).
  - Trace route (per-hop SNR bars, both directions).
  - Ping & range test (success %, RTT, SNR sparkline).
  - Signal (last RSSI/SNR, noise floor sparkline, packet rate).
  - Packet log (time, type, route, hops, SNR, summary).
  - Airtime (TX/RX time, regulatory duty-cycle gauge, queue).
  - Repeater admin (login, status, neighbours).
  - GPS (fix, satellites, position, distance and bearing to contacts).
  - Map (braille canvas, self and contacts, zoom).
- **My card**: full-screen QR overlay of `meshcore://contact/add?name=<name>&public_key=<64 hex>&type=1`, scannable by the official app, with the name and short key below.
- **Pairing**: full-screen PIN in big digits as soon as pairing starts; gone when it ends.
- **Hibernate**: leaves a badge on the panel (name, my-card QR, "off — press B") and wakes on B.
- **Timestamps are fixed** ("14:02", "yesterday"), never "x s ago", so the clock alone never forces a redraw beyond its minute tick.
- **First boot**: a setup flow (region, name, time from GPS or the app). It can be skipped, and it comes back from Settings → System.

## 7. Two-button text entry

- Text box at the top, candidate strip at the bottom; the byte count and the 160-byte limit are shown live.
- A click / double click moves through the candidates; a long A inserts the current one. B click deletes the last character; B on empty text cancels.
- Candidate order: quick replies (empty text only); up to 4 completions of the word being typed, your recent words first, then a built-in French and English list (about 600 common and messaging words to start; the goal stays about 2,000 per language); `space`; letters by frequency (`e é è ê a à â s i t n r u l o d c ç m p…`); digits and punctuation; the final action (`Send`, `Save`, `Add`) last, one double click away. A quick reply moves the cursor straight to `Send`.
- The first letter of a sentence is offered in upper case. `#channel` names: lower-case ASCII letters, digits and `-`.
- Long messages still come from the phone app.

## 8. Notifications

- **Blue LED**: blinks briefly every 5 s while something is unread.
- **Buzzer**: off / quiet / on. A short tune for a DM, another one for a channel, a beep for a delivery; only when no app is connected (like the C++).
- We never switch screens on the user's behalf while they are navigating; a new DM shows the banner (§4).

## 9. Local history and unread

- The last 200 messages (DMs and channels, received and sent from the screen) are kept in flash, independently of the protocol's offline queue: peer, direction, time, status, text.
- A message becomes read when it is opened on the device **or** when the app fetches it with `SYNC_NEXT_MESSAGE`.

## 10. E-paper refresh

- Partial refresh for everything; a full refresh is decided by a **debt** (`mc_ui::refresh`):
  - each partial refresh adds 100 (150 once the debt has reached 1,000);
  - a full refresh brings the debt back to 0;
  - once the debt reaches 1,000, a full refresh runs 60 s after the last input;
  - at 3,000, the next screen change is a full refresh;
  - a screen change right after sending something is a full refresh.
- No refresh when the frame has not changed (hash of the panel bytes).
- The clock redraws once a minute; the battery percentage redraws only when it moves by 5 points or more.
- Measured (U0): partial refresh visible 549 ms after the input, full refresh 2.0 s.

## 11. Emulator and verification

- `mc ui` runs the real `mc-ui` App in a terminal at the device's grid size, hosted by `mc-sim` (two virtual peers from U3 on). Keys: `j`/↓ = A click, `k`/↑ = A double click, Enter = A long, Space (held) = A hold, Esc/Backspace = B click, `h` = B double click, `l` = B long, `g` = switch grid, `q` = quit.
- `mc ui --png <dir>` renders every screen through mousefood into PNG files: the exact panel pixels, overlays included.
- Snapshot tests (`insta`) of every screen and state at both grid sizes.
- On the device (U6): navigation with the two buttons, a message composed and delivered with its tick, tools on real traffic, backlight and potentiometer, no packet lost during a refresh (M5 log compared with the Heltec's).
