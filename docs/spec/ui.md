# ThinkNode M5 interface

Source of truth for `mc-ui`. On-screen text **in English**. 200×200 black and white e-paper screen, two buttons, brightness potentiometer, blue LED, buzzer.

## 1. Principles

- **Both buttons always keep the same meaning**, recalled by a help bar at the bottom of the screen.
- **The whole screen is used** (the C++ layout is an enlarged 128×64 screen, which leaves 45 % empty).
- **Readable accents** (u8g2 fonts with Latin-1 and Latin Extended); a character without a glyph is shown as a diamond, not as a block.
- **The screen never blocks the radio or Bluetooth**: waiting for a refresh to finish is asynchronous.
- **Nothing disappears from the app on its own**: the offline message queue remains the protocol's (`docs/spec/companion.md` §5).

## 2. Buttons

| Button | Short press | Long press (≥ 600 ms) | Double press (< 300 ms) |
|---|---|---|---|
| A (GPIO21, bottom) | next / down | open / confirm | previous / up |
| B (GPIO14, top) | back (home from the first level) | turns the backlight on or off | — |

- The first press while the screen's backlight has timed out only turns the backlight back on, if it was on before.
- For 8 s after boot, a long press on A opens the recovery screen (settings reset, migration restart).

## 3. Backlight and potentiometer

- The backlight stays on once enabled, so that its brightness can be adjusted with the potentiometer.
- The potentiometer is not readable by the firmware (`docs/spec/m5.md`): no dead zone is possible at the bottom of its travel. Turning it 100 % off is guaranteed by button B and the auto-off, which cut the LED power (IO5), whatever the knob position. The header always shows the backlight state (Tanguy, 2026-11-06).
- Adjustable auto-off: 30 s, 2 min (default), never; the timer restarts on each press.
- Light bulb icon in the header while it is on.

## 4. Header (all screens)

`HH:MM` · unread (envelope + count) · Bluetooth (connected, waiting, off) · backlight · **battery as a percentage** (charging symbol when USB is plugged in; `docs/spec/m5.md` §3).

## 5. Screens

| Screen | Content |
|---|---|
| **Home** | Large clock, date. Unread: DMs and channels. Preview of the last message (sender, start of the text, age). Battery as a percentage and as a voltage. Bluetooth: name of the connected app, or "Pair: 123456" during pairing. Network: preset (e.g. "EU Narrow · fr"), last repeater heard, noise floor. GPS: fix and satellites if enabled |
| **Inbox** | Conversations (contacts and channels), sorted by last message, with an unread dot and a one-line preview |
| **Conversation** | Bubbles (received on the left, sent on the right) with line wrapping. Sending: `sending…`, `delivered ✓ 2.4 s` (round trip), `failed`. Receiving: hops (`direct` or `3 hops`) and SNR. A long = Quick reply |
| **Quick reply** | 8 canned replies (default: OK, Yes, No, On my way, Call me, Where are you?, Running late, Thanks) and "Send my position". Editable with the CLI command `set reply.N <text>`. Sending from the device: 3 direct attempts, then `RESET_PATH` and flood (deliberate deviation 6) |
| **Network** | Nodes heard (name, type, hops, SNR, age), repeaters first. A long = zero-hop discovery (at most once every 5 s). Airtime over the last hour and duty cycle |
| **My card** | QR `meshcore://contact/add?name=<name>&public_key=<64 hex>&type=1`, scannable by the official app; name and shortened key below it |
| **Pairing** | Full screen, PIN in very large digits, as soon as pairing starts; disappears when it ends |
| **Settings** | Region (presets from `config/regions.toml`), Bluetooth, GPS, buzzer, backlight duration, send an advert (zero-hop or flood), hibernation |

Navigation: Home → (A short) Inbox → Network → My card → Settings → Home; A long opens the selected item; B goes back.

## 6. Notifications

- **Blue LED**: blinks briefly every 5 s while something is unread.
- **Buzzer**: a short tune for a DM, another one for a channel, a beep for an ACK; only when no app is connected (like the C++); can be muted in settings.
- A new DM shows its preview on Home; we never switch screens on the user's behalf while they are navigating.

## 7. Local history and unread

- The last 64 messages (DMs and channels, received and sent from the screen) are kept in flash, independently of the protocol's offline queue.
- A message becomes read when it is opened on the device **or** when the app fetches it with `SYNC_NEXT_MESSAGE`.

## 8. E-paper refresh

- Partial refresh for navigation and updates; full refresh every 30 partials, every hour, and on each return to Home after more than 10 min.
- Clock updated every minute; no seconds counter; ages are shown in minutes.
- No redraw if the image has not changed (buffer hash).
- Battery: moving average, updated at most once a minute.

## 9. Verification

- `mc ui --out <dir>` renders each screen, in scripted states, as PNG; these reference images are compared in tests.
- On the device (MC7): reading a DM, quick reply with delivery tick, QR added in the official app, backlight kept on and adjusted with the potentiometer, battery percentage accurate to ±5 %, no packet lost during a refresh.
