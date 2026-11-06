# Hardware measurements and tests

Log of tests on the real devices. Each entry: date, setup, command, result.

## 2026-11-05 — `mc airtest`: mc-air against MeshCore C++ v1.18 (MC1)

- Setup: Heltec V4 running `companion_radio_usb` v1.18.0 (reference, `/dev/cu.usbmodem141101`); ThinkNode M5 running `companion_radio_usb` v1.18.0-81dbae5 (device under test, `/dev/cu.wchusbserial120`). Region fr: 869.618 MHz, 62.5 kHz, SF8, CR 4/8.
- Command: `mc airtest --ref /dev/cu.usbmodem141101 --dut /dev/cu.wchusbserial120`.
- Everything zero-hop (direct route, empty path): nothing is forwarded on the network.
- Result: **10 checks out of 10** on the first try.

| Check | Result |
|---|---|
| Advert signed by mc-air (orlp test identity) accepted by the C++ | contact added (ADVERT) |
| DM encrypted by mc-air, read by the C++ | identical text; `path_len` 0xFF (direct) |
| C++ ACK equal to the CRC computed by mc-air | identical, 6 bytes |
| `SEND_TXT_MSG` on the C++ sent direct, not flood | `SENT` with flood = 0 |
| DM from the C++, decrypted by mc-air | identical text |
| ACK CRC computed by mc-air equal to the one reported by the C++ in `SENT` | identical |
| ACK built by mc-air recognized by the C++ | `SEND_CONFIRMED`, round trip 824 ms |
| Advert from the C++ verified by mc-air | signature ok, identical name |
| Channel datagram from the C++ (zero-hop) decrypted by mc-air | identical |
| Channel message from mc-air read by the C++ | identical |

Other findings:
- The M5 reports 869618 kHz in `SELF_INFO`: deviation 2 as predicted by the study (869617) does not occur in the usual bands (ADR-0001 corrected).
- `mc sniff` on the M5 for 2 min (22:15): no packet heard, the network was quiet at that time.

## 2026-11-06 — `mc airtest` with oxymesh as the device under test (MC4)

- Setup: Heltec V4 running `companion_radio_usb` v1.18.0 (reference, `/dev/cu.usbmodem2141101`); ThinkNode M5 running **oxymesh** `mc-m5` over USB (device under test, `/dev/cu.wchusbserial2120`). Same radio settings as above.
- Command: `mc airtest --ref /dev/cu.usbmodem2141101 --dut /dev/cu.wchusbserial2120`.
- In this run, "the C++ firmware" in the check names is oxymesh: the test identity talks to the device under test, the Heltec only listens and injects raw packets.
- Result: **10 checks out of 10**, the same checks as the C++ run above. ACK round trip 692 ms (824 ms with the C++ firmware on the M5).

## 2026-11-06 — `mc duo`: oxymesh (M5) and C++ (Heltec) talk to each other (MC4)

- Setup: A = ThinkNode M5 running oxymesh `mc-m5` (`/dev/cu.wchusbserial210`); B = Heltec V4 running `companion_radio_usb` v1.18.0 (`/dev/cu.usbmodem1101`). Region fr. No injected frames: each device builds its own packets from companion commands. Everything zero-hop.
- Command: `mc duo --a /dev/cu.wchusbserial210 --b /dev/cu.usbmodem1101 --pings 10`.
- Result: **12 checks out of 12**.

| Check | A → B | B → A |
|---|---|---|
| Zero-hop advert reported (`ADVERT`) | after 2.5 s | after 1.2 s |
| DM sent direct (`SENT`) | ok | ok |
| DM received (`MSG_WAITING`, `SYNC_NEXT_MESSAGE`) | identical text, `path_len` 0xFF, SNR 12.0 dB | identical text, `path_len` 0xFF, SNR 13.25 dB |
| Delivery confirmed (`SEND_CONFIRMED`) | round trip 1125 ms | round trip 1257 ms |
| Channel datagram, zero hop (`CHANNEL_DATA_RECV`) | `path_len` 0xFF, SNR 13.0 dB | `path_len` 0xFF, SNR 12.25 dB |
| Telemetry requests as pings | 10/10, RTT 1035 / 1040 / 1047 ms | 10/10, RTT 1008 / 1015 / 1025 ms |

Findings:
- **Bug, fixed:** oxymesh reported a zero-hop channel message or datagram with `path_len` 0x00. The C++ firmware reports 0xFF for any direct packet (M:572, 617). companion.md §5 now says so, and an mc-sim test covers it.
- **Test artifact:** GRP_DATA has no nonce. The same data on the same channel within one second gives byte-identical packets, and the receiver drops the second one as already seen (the C++ firmware does the same). `mc duo` puts the sender in the data.
- **Open:** telemetry pings that start right after `SET_OTHER_PARAMS` (which `mc duo` uses to allow telemetry) lose their second exchange.
  - Without a pause, in 6 runs, A → B lost ping 2 in 5 and B → A in 2; the other pings were all answered.
  - The lost packet is always the one oxymesh sends and the C++ firmware never sees: A counts it as sent, while B's receive counter and `LOG_RX_DATA` miss it, with no CRC error.
  - Zero-hop datagrams sent every 600 ms during the 4 s after a prefs save, on either device, all arrived (48/48), so it is not simply "deaf after a flash write".
  - With 100 to 300 ms before each ping: 0 lost in 2 runs. With 3 s between the prefs change and the first ping: 0 lost in 5 runs (100 pings). `mc duo` now waits 3 s.
  - Not reproduced between two oxymesh nodes in mc-sim. Cause unknown; next step is a run with the C++ firmware on both ends.

## 2026-11-06 — the interface on the M5 (U6, part 1)

- Firmware `mc-m5` with `mc-ui`: 64 KB heap, one Ratatui terminal drawing into the panel frame, overlays, refresh-debt policy, gestures from the two buttons, light auto-off, unread LED, hibernation.
- Image 900,384 bytes, against a 1.75 MB app partition.
- `mc duo --a /dev/cu.wchusbserial210 --b /dev/cu.usbmodem1101 --pings 3 --reboot`: 13/13 with the interface running in the same loop as the radio.
- Checked by hand (Tanguy): a quick reply ("On my way") written on the M5 reached the Heltec (flood, SNR 12.25 dB) and showed "delivered" on the M5; the Heltec's answer (`mc send`, direct, acknowledged in 1.1 s) brought up the new-message banner.
- Still to check by hand: the tools on real traffic, light and potentiometer, the unread LED, no packet lost during a refresh.

## 2026-11-06 — offline queue in flash, screen history (U2)

- Firmware with the offline queue and the 200-message history in the `mcdata` partition (index in RAM), prefs record v2. The M5's v1 prefs record (128 bytes) was read back: same name, key and radio after flashing.
- `mc duo --a /dev/cu.wchusbserial210 --b /dev/cu.usbmodem1101 --pings 3`: 12/12, every DM and sync now going through flash.
- `mc duo … --pings 0 --reboot`: Bob writes to the M5, the M5 reboots before the app fetches it, and the DM is still delivered after the reboot (11/11).
- `Device`: 146,232 bytes (147,240 before): the 40 KB history, the heard-nodes ring (2.8 KB) and the packet log (1 KB) cost less than the 47 KB queue that moved to flash.
- After the settings API refactor (U2, part 2): `mc duo … --pings 3 --reboot` 13/13; the companion arms now go through the shared setters.

## 2026-11-06 — U0: Ratatui on the M5's e-paper (ADR-0008)

- Setup: ThinkNode M5 running `ui-spike` (`firmware/esp32s3/src/bin/ui-spike.rs`): `mc_ui::demo` (header, list with a reversed selection, wrapped paragraph, sparkline, footer) drawn by Ratatui through mousefood into the panel frame, `esp-alloc` heap of 96 KB with `internal-heap-stats`. A frame every 4 s, grid switch every 6 frames. Logs `local/ui-spike-*.log`.
- Flash: the spike image is 346 KB with esp-hal and the fonts; the companion `mc-m5` is 493 KB, for a 1.75 MB app partition.

| | Large 25×15 | Compact 33×20 |
|---|---|---|
| Heap, steady (two cell buffers) | 21,008 B | 36,968 B |
| Heap, peak while drawing | 22,584 B | 39,376 B |
| Draw, whole screen | 6 ms (15 ms the first time) | 6 ms (11 ms the first time) |
| Frame sent to the panel (SPI 4 MHz) | 20 ms | 20 ms |
| Partial refresh until BUSY drops | 542 ms | 542 ms |
| Input to visible | 549 ms | 549 ms |

- Full refresh: 2007 ms.
- First version, with `Layout`: peak 55.5 KB (large) and 75.2 KB (compact), draw 21 ms. The constraint solver accounted for about 30 KB and 15 ms per draw.
- First version, with `..EmbeddedBackendConfig::default()`: 3.8 KB more heap after each grid switch. The default config builds the 6×10 font atlas and leaks it (`FontAtlas::leak`); it now has zero leak (host counting allocator: 0 B after create, draw and drop).
- RAM of `mc-m5` (static): `.data` 156.5 KB (of which `Device` 147 KB), `.bss` 16.8 KB, `.rwdata` 10.7 KB. The stack takes the rest of the 334 KB main DRAM (≈ 158 KB); `dram2` (72 KB) is free for Bluetooth.

## 2026-11-05 — one-hour fuzz runs

| Target | Executions | Duration | Crashes |
|---|---|---|---|
| `air_decode` (MC1) | 351,327,535 | 3601 s | 0 |
| `cmd_decode` (MC2, 2026-11-06) | 388,902,314 | 3601 s | 0 |
