# ADR-0001 — Strict compatibility with MeshCore v1.18

- Status: accepted
- Date: 2026-11-05

## Context

The goal is a Rust firmware for the ThinkNode M5 that replaces the MeshCore firmware without breaking anything: it must talk to original MeshCore nodes over the air (the French repeater network) and remain usable with the official MeshCore app, meshcore.js and loranet's web app. The only visible gain expected is the on-device interface.

## Decision

- **Reference**: MeshCore `companion-v1.18.0`, commit 81dbae5 (2026-10-15). The `docs/spec/air.md` and `docs/spec/companion.md` specs cite lines from this commit; the C++ code takes precedence over the MeshCore docs when they disagree.
- **Over the air**: the same bytes transmitted as the C++ firmware under the same conditions, the same packets accepted (including older variants still tolerated by v1.18).
- **Companion protocol**: the same responses, byte for byte, to the same commands. The only differences allowed are these **deliberate deviations**, invisible to the app:
  1. the offline message queue survives a reboot (kept in flash);
  2. the frequency is reported exactly, in whole kHz. The C++ recomputes it as a `float` (`(uint32)(freq × 1000)`): checked value by value, the result is identical in the 433-435, 863-870 and 902-928 MHz bands (the real M5 does return 869618), and only loses 1 kHz for about 1 % of frequencies outside those bands;
  3. a channel name in `CHANNEL_INFO` is zero-padded (the C++ leaves leftover bytes there);
  4. no truncation of 174- to 176-byte frames over Bluetooth (MTU of at least 179 negotiated);
  5. the random PIN is drawn on each pairing rather than on each boot;
  6. a message sent from the device's screen is retried by the firmware if it is not acknowledged (no app is there to do it); a message sent by the app keeps the C++ rule (no retry by the firmware).
- Any new deviation goes through an ADR that amends this one.

## Consequences

- We inherit MeshCore's cryptographic weaknesses (AES-128-ECB, 2-byte MAC, static keys): that is the price of compatibility. Strong encryption remains the app's job (ln-msg in loranet).
- `mc conform` compares our responses with those of the Heltec running C++: any difference outside the list above is a bug.
- Rejected: an "enhanced" mode negotiated between Rust nodes. It would fragment the network for a gain nobody asked for.
