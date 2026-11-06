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

## 2026-11-05 — one-hour fuzz run

| Target | Executions | Duration | Crashes |
|---|---|---|---|
| `air_decode` (MC1) | 351,327,535 | 3601 s | 0 |
