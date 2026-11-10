# Heltec WiFi LoRa 32 V4

Second board (ADR-0009). Sources: MC = MeshCore v1.18 (`variants/heltec_v4/`, MIT), MT = Meshtastic (`variants/esp32s3/heltec_v4/`, GPL: facts only), LN = loranet's firmware for this board (same author).

## 1. Microcontroller

ESP32-S3R2: 16 MB flash, 2 MB PSRAM (unused so far), 240 MHz. **Native USB** (USB-Serial-JTAG, `303a:1001`): the companion protocol runs on it, `<`/`>` framing as on the M5's UART. A frame no computer reads within 5 ms is dropped, so the node runs on unplugged.

## 2. Pinout

| Block | Signal | GPIO / value | Source |
|---|---|---|---|
| SX1262 | SCK / MOSI / MISO | 9 / 10 / 11 | MC, MT, LN |
| | NSS / RESET / BUSY / DIO1 | 8 / 12 / 13 / 14 | MC, MT, LN |
| | TCXO | 1.8 V on DIO3 | MC, MT |
| | RF switch | DIO2 drives the front end's TX/RX path | MC, MT |
| | current limit, RX patch | 140 mA; register 0x08B5 bit 0 set (`SX126X_REGISTER_PATCH`) | MC |
| Front end | power | GPIO7 high (LDO) | MC, MT, LN |
| | type | GPIO2 read at boot with no pull: high = KCT8103L (V4.3), low = GC1109 (V4.2) | MC, LN |
| | GC1109 | GPIO2 = enable, GPIO46 = CPS (high = full PA in TX) | MC, MT |
| | KCT8103L | GPIO2 = CSD (enable), GPIO5 = CTX (in RX: low = LNA on, high = bypass; high in TX) | MC, MT |
| | gain | +11 dB up to 15 dBm in, +10 at 16-17, +9 at 18-19, +7 at 21 (GC1109, measured); 28 dBm out at most | MT |
| OLED | controller | SSD1306/SSD1315, 128×64, I2C 0x3C on SDA 17 / SCL 18, reset GPIO21 | MC, LN |
| | power | Vext, GPIO36, **active low** (MT, LN; MC's `PIN_VEXT_EN_ACTIVE=HIGH` is wrong for this: checked on a V4.3, 2026-11-10) | MT, LN, test |
| Button | user (PRG) | GPIO0, active low, pull-up; wake-up source | MC, MT |
| LED | white | GPIO35 (MC: TX light) | MC, MT |
| Battery | voltage | ADC GPIO1, 2.5 dB attenuation, enabled by GPIO37 high; multiplier 5.42 (MC) or 4.9 × 1.045 (MT): to calibrate | MC, MT |
| GPS header | UART 9600 | 38 / 39 (MC calls 38 RX, MT calls 38 the line to the GPS: to settle with a module), EN 34 active low, RESET 42 active low | MC, MT |
| Clock chip, buzzer | — | none on the board | — |

## 3. Front end and power

- Around each transmission, like `LoRaFEMControl`: TX mode (full PA) before it, RX mode after it (the LNA as `fem_rxgain` says, on the KCT8103L), shut down before hibernating.
- `tx` stays the SX1262 setting, as in the C++ (default 10, maximum 22). Within the EU profile the firmware lowers it until the radiated power, setting plus gain, fits the sub-band's limit (`docs/spec/air.md` §4.5): at most 16 dBm in g3 (26 dBm out).
- `radio.fem.rxgain` in the console: `docs/spec/companion.md` §6.1. The board name follows the chip found: "Heltec V4.3 OLED" or "Heltec V4 OLED".

## 4. Flash

`firmware/esp32s3/partitions-heltec-v4.csv`: nvs, otadata, `spiffs` (0xC90000, 3.4 MB) and coredump at the C++'s places (arduino-esp32 `default_16MB.csv`, read from a V4.3 running C++ v1.18 on 2026-11-10: nvs 0x9000, otadata 0xE000, app0 0x10000 and app1 0x650000 of 6.25 MB each, spiffs 0xC90000, coredump 0xFF0000); the app at 0x10000; `mcdata` (768 KB) at 0x650000, inside the C++'s second OTA slot. First boot imports the C++ node from `spiffs` like the M5 (ADR-0006).

`scripts/flash.sh heltec-v4 <port>` builds and flashes with this table.

## 5. Screen

Status pages, one button (`docs/spec/ui.md` §12). The screen goes dark after 60 s without a press, and lights when a phone starts pairing; the first press of a dark screen only lights it. The LED blinks while something is unread and lights while transmitting.
