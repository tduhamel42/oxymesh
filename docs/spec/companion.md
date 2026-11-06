# MeshCore companion protocol, device side (v1.18)

Source of truth for `mc-proto` and the companion layer of `mc-node`. Reference: MeshCore `companion-v1.18.0`, commit 81dbae5 (ADR-0001). Integers are little-endian. Positions `[i]` count the command code as `[0]`.

Abbreviations: `M` = `examples/companion_radio/MyMesh.cpp`, `M.h` = `MyMesh.h`, `DS` = `examples/companion_radio/DataStore.cpp`, `NP` = `examples/companion_radio/NodePrefs.h`, `BCM` = `src/helpers/BaseChatMesh.cpp`, `BLE` = `src/helpers/esp32/SerialBLEInterface.cpp`, `ASI` = `src/helpers/ArduinoSerialInterface.cpp`.

## 0. Constants

| Item | Value | Source |
|---|---|---|
| `FIRMWARE_VER_CODE` | 14 | M.h:7 |
| Version, build date | `"v1.18.0"`, `"15 Oct 2026"` (release builds use `v1.18.0-81dbae5`, `15-Oct-2026`) | M.h:9-15, `build.sh:133-150` |
| Frame | 176 bytes at most, in both directions | `src/helpers/BaseSerialInterface.h:5` |
| M5 build | 350 contacts, 40 channels, queue of 256, PIN 123456 (→ random, §1.1), 22 dBm max | `variants/thinknode_m5/platformio.ini:174-181` |
| Manufacturer (M5) | `"Elecrow ThinkNode M5"` | `variants/thinknode_m5/ThinknodeM5Board.cpp` |
| Temporary contacts ("anon") | 8, at indices 0..7 | `BaseChatMesh.h:41, 97-100` |
| Expected ACKs | 8, in a ring | M.h:285 |
| Stored advert paths | 16 | M.h:289 |
| Connections (keep-alive) | 16 | `BaseChatMesh.h:44` |
| Text | 160 bytes | `BaseChatMesh.h:8` |

**Dispatch rule** (M:1055-2062): a command whose length precondition fails falls through to the last `else` and gets `ERR 1` (UNSUPPORTED_CMD), not `ERR 6`. Exceptions, where the frame's shape is checked inside the branch and refused with `ERR 6`: SEND_CHANNEL_DATA (len < 4 or invalid `path_len`, M:1199-1211), SEND_RAW_DATA (fewer than 4 payload bytes, M:1563), SEND_TRACE_PATH (hashes inconsistent with `flags`, M:1817). Per-command details are in §10.

### Error codes (M:135-140)

| Code | Name |
|---|---|
| 1 | UNSUPPORTED_CMD (also: length precondition) |
| 2 | NOT_FOUND |
| 3 | TABLE_FULL |
| 4 | BAD_STATE |
| 5 | FILE_IO_ERROR |
| 6 | ILLEGAL_ARG |
| response 15 | DISABLED |

`ERR` = `[0x01, code]`. `OK` = `[0x00]` alone.

## 1. Interfaces

### 1.1 Bluetooth (BLE)

- Nordic UART service `6E400001-B5A3-F393-E0A9-E50E24DCCA9E`; RX `…0002…` (write, encryption + MITM required); TX `…0003…` (read and notify, encryption + MITM) (BLE:7-9, 45-51). The service UUID is advertised (BLE:53).
- Advertising (Arduino-ESP32 `BLEAdvertising` defaults): flags and the 128-bit service UUID in the advertisement, the name in the scan response. A name longer than the scan response allows (29 bytes) goes out shortened (AD type 0x08). The GAP device name keeps it whole in the C++; ours keeps the first 22 bytes (deviation 11).
- Name: `"MeshCore-" + node name`, set at boot (a name change takes effect on reboot). A `"@@MAC"` name is replaced by the 12 hex digits of the MAC address (BLE:16-24).
- Security: LE Secure Connections + MITM + bonding, passkey displayed by the device (the user types it on the phone) (BLE:31-33, 58-75). Bonds are in NVS, erased by `FACTORY_RESET`.
- PIN (`examples/companion_radio/main.cpp:188-202`): `prefs.ble_pin` if non-zero; otherwise, with a screen and the build PIN 123456, a **random PIN in 100000..999999 drawn at each boot** and shown on screen (the M5's case); otherwise the build PIN. The same PIN serves every pairing until the next boot. `DEVICE_INFO` reports `prefs.ble_pin` (0 in random mode), not the active PIN (M:1064). `SET_DEVICE_PIN` only saves the setting (M:1847): the C++ applies it at the next boot; we apply it at once (deviation 9).
- "Connected" means **authenticated**: outgoing frames are dropped until pairing has succeeded; on failure, disconnect and restart advertising after 1 s (BLE:77-88, 174).
- One write = one frame; one notification = one frame. No extra framing.
- Input: queue of 4 frames; a frame > 176 bytes or a full queue → frame dropped (BLE:114-129).
- Output: queue of 4 frames (full → dropped); at most one notification every **60 ms** (BLE:168-209).
- Disconnection: queues flushed, advertising restarts after 1000 ms. The state machine's state is **not** reset (`app_target_ver`, offline queue, contact iteration) (BLE:104-110, 218-237). No advertising while connected.

### 1.2 USB / serial (ASI:24-73)

- App → device: `'<'` (0x3C), u16 length, payload. Device → app: `'>'` (0x3E), u16 length, payload.
- Bytes before `'<'` ignored; length 0 → reader reset; bytes beyond 176 read and dropped (frame truncated to 176); no inter-byte timeout.
- Always "connected": notifications are always sent. 115200 baud (on the M5, UART0 through the CH340 bridge).

### 1.3 Several interfaces (`src/helpers/MultiSerialInterface.h:117-198`)

Each outgoing frame goes out on **all** active interfaces; the first available frame is read; "connected" if any one is.

## 2. Commands

`SENT` (response 6) is always 10 bytes: `[1]` 1 = flood / 0 = direct, `[2..5]` expected ACK or tag, `[6..9]` estimated timeout in ms (u32), computed as follows (M:888-896):
- flood: `500 + 16 × airtime_ms`;
- direct: `500 + (airtime × 6 + 250) × (hops + 1)`, hops = `path_len & 63`.

| Code | Name | Request | Response and effects | Source |
|---|---|---|---|---|
| 1 | APP_START | len ≥ 8; `[1..7]` reserved, `[8..]` app name | SELF_INFO (§3.1); interrupts a contact iteration | M:1076-1117 |
| 2 | SEND_TXT_MSG | len ≥ 14; `[1]` txt_type, `[2]` attempt, `[3..6]` ts, `[7..12]` key prefix (6), `[13..]` text | type 0: SENT with expected ACK (entered into the ring of 8). Types 1 and 3: ts replaced by the device's unique time, ACK 0. Unknown contact → ERR 2; other type → ERR 1; failure or text too long → ERR 3 | M:1132-1177 |
| 3 | SEND_CHANNEL_TXT_MSG | `[1]` txt_type (0), `[2]` channel, `[3..6]` ts, `[7..]` text (may be empty); no length check: below 7 bytes, a negative text length is passed to `memcpy` (undefined behavior, BCM:528; deliberate deviation 7) | **OK** (not SENT). Transmitted: `"<name>: <text>"` truncated to 160. Invalid channel or send failure → ERR 2; type ≠ 0 → ERR 1 | M:1178-1197, BCM:518-537 |
| 4 | GET_CONTACTS | `[1..4]` since, read only if len ≥ 5 (else 0) | CONTACTS_START (2) `[1..4]` = total count; one CONTACT (3) per contact with `lastmod > since`; END_OF_CONTACTS (4) `[1..4]` = highest lastmod sent (0 if none). One contact per loop iteration, at the interface's pace. New request during the iteration → ERR 4 | M:1238-1258, 2388-2409 |
| 5 | GET_DEVICE_TIME | — | CURR_TIME (9) `[1..4]` = Unix time | M:1281-1286 |
| 6 | SET_DEVICE_TIME | len ≥ 5; `[1..4]` time | OK if ≥ current time, otherwise **ERR 6** (the clock never goes back) | M:1287-1296 |
| 7 | SEND_SELF_ADVERT | `[1]` optional: 1 = flood (default scope), otherwise zero-hop | OK or ERR 3; position included if `advert_loc_policy ≠ 0` | M:1297-1316 |
| 8 | SET_ADVERT_NAME | len ≥ 2; name (31 bytes max) | OK; prefs saved | M:1259-1265 |
| 9 | ADD_UPDATE_CONTACT | len ≥ 36; contact (§3.2) from `[1]`; lat/lon if len ≥ 144; lastmod if len ≥ 148 | field-by-field update, or add (sync_since = 0); lastmod = value supplied or now. OK or ERR 3 (full). Fields are read up to `[135]` even if the frame is shorter (bytes outside the frame: §10). Contacts write deferred by 5 s | M:1328-1348, 194-217 |
| 10 | SYNC_NEXT_MESSAGE | — | head of the offline queue as is (7/8/16/17/27) or NO_MORE_MESSAGES (10) | M:1415-1423 |
| 11 | SET_RADIO_PARAMS | `[1..4]` frequency kHz, `[5..8]` BW Hz, `[9]` SF, `[10]` CR, `[11]` repeat (optional, v9+, read if len ≥ 12); no length check | bounds: 150000..2500000, 7000..500000, 5..12, 5..8. repeat ≠ 0 requires an allowed frequency, otherwise ERR 6. **Without the repeat byte, repeat is disabled.** Applied and saved; OK | M:1424-1459 |
| 12 | SET_RADIO_TX_POWER | `[1]` i8 dBm; no length check | −9..22 on the M5, otherwise ERR 6; OK | M:1460-1469 |
| 13 | RESET_PATH | len ≥ 33; key | out_path_len = 0xFF; OK / ERR 2 | M:1317-1327 |
| 14 | SET_ADVERT_LATLON | len ≥ 9; lat, lon i32 ×1e6, optional alt (ignored) | bounds, otherwise ERR 6; OK | M:1266-1280 |
| 15 | REMOVE_CONTACT | key, read without length check | removes the contact and its stored advert; OK / ERR 2 | M:1349-1358 |
| 16 | SHARE_CONTACT | key, read without length check | retransmits the stored advert zero-hop, transport codes {0,0}; OK / ERR 3 / ERR 2 | M:1359-1370, BCM:570-582 |
| 17 | EXPORT_CONTACT | key; len < 33 = self | EXPORT_CONTACT (11) + raw packet (§3.3); ERR 2 | M:1379-1408 |
| 18 | IMPORT_CONTACT | len > 98; raw advert packet | replayed as if received by flood (auto-add and anti-replay apply). OK sent before processing; ERR 6 if it is not an advert | M:1409-1414, BCM:588-601 |
| 19 | REBOOT | `[1..6]` = `"reboot"` (compared whatever the length) | no response; writes pending contacts, reboots | M:1512-1516 |
| 20 | GET_BATT_AND_STORAGE | — | BATT_AND_STORAGE (12): mV u16, used KB u32, total KB u32 | M:1517-1527 |
| 21 | SET_TUNING_PARAMS | rx_delay ×1000 u32, airtime_factor ×1000 u32; no length check | saved; OK | M:1470-1480 |
| 22 | DEVICE_QUERY | len ≥ 2; `[1]` app_target_ver | stores app_target_ver; DEVICE_INFO (§3.1) | M:1056-1075 |
| 23 | EXPORT_PRIVATE_KEY | — | PRIVATE_KEY (14) + 64 bytes | M:1528-1536 |
| 24 | IMPORT_PRIVATE_KEY | len ≥ 65; 64-byte key | checked (pub[0] ∉ {00, FF}, ECDH self-test); saved; OK; contacts reloaded. ERR 6 invalid, ERR 5 write | M:1537-1556, `src/Identity.cpp:67-109` |
| 25 | SEND_RAW_DATA | len ≥ 6; `[1]` path_len (invalid, 0xFF included → ERR 1), path, payload (fewer than 4 bytes → ERR 6) | direct RAW_CUSTOM; OK / ERR 3 | M:1557-1576 |
| 26 | SEND_LOGIN | len ≥ 33; key, password (15 usable chars) | SENT with tag = **first 4 bytes of the key**; cancels pending requests. Result: 0x85 or 0x86 | M:1577-1598, BCM:603-636 |
| 27 | SEND_STATUS_REQ | len ≥ 33; key | SENT (tag); result 0x87 (matched by key prefix) | M:1630-1650, BCM:690-715 |
| 28 | HAS_CONNECTION | len ≥ 33; key | OK if a keep-alive connection is active, otherwise ERR 2 | M:1741-1747 |
| 29 | LOGOUT | len ≥ 33; key | removes the connection; always OK | M:1748-1751 |
| 30 | GET_CONTACT_BY_KEY | key, read without length check | CONTACT (3) / ERR 2 (also searches temporary contacts) | M:1371-1378 |
| 31 | GET_CHANNEL | len ≥ 2; `[1]` index | CHANNEL_INFO (18): `[1]` index, `[2..33]` name (32), `[34..49]` secret (16); 50 bytes; ERR 2 if index ≥ 40 | M:1752-1766 |
| 32 | SET_CHANNEL | len 50..65; index, name (32), secret (16) | hash = SHA256(secret)[0]; saved immediately; OK / ERR 2. **len ≥ 66 (32-byte key) → ERR 1** | M:1767-1780 |
| 33 | SIGN_START | — | SIGN_START (19): `[1]` 0, `[2..5]` max size 8192 | M:1781-1792 |
| 34 | SIGN_DATA | len > 1; chunk | OK; ERR 4 without SIGN_START; ERR 3 beyond 8 KB | M:1793-1800 |
| 35 | SIGN_FINISH | — | SIGNATURE (20) + 64 bytes / ERR 4 | M:1801-1812 |
| 36 | SEND_TRACE_PATH | len > 10 and len − 10 < 179 (always true); tag u32, auth u32, flags, hashes of 2^(flags & 3) bytes | SENT (flag 0, app's tag, direct timeout); ERR 6 if `(len − 10)` is not a multiple of 2^(flags & 3) or if `(len − 10) >> (flags & 3)` > 64; ERR 3 | M:1813-1838 |
| 37 | SET_DEVICE_PIN | len ≥ 5; u32 | 0 or 100000..999999; saved (active on reboot); OK / ERR 6 | M:1839-1852 |
| 38 | SET_OTHER_PARAMS | `[1]` manual_add (read without length check); optional, if len ≥ 3, 4, 5: `[2]` telemetry (bits 0-1 base, 2-3 location, 4-5 environment), `[3]` advert_loc_policy, `[4]` multi_acks | saved; OK | M:1488-1503 |
| 39 | SEND_TELEMETRY_REQ | len ≥ 36: `[4..35]` key → remote; len == 4 → self; other → ERR 1 | remote: SENT then 0x8B. Self: **immediate 0x8B notification** (voltage, MCU temperature, sensors as CayenneLPP) | M:1680-1719 |
| 40 | GET_CUSTOM_VARS | — | CUSTOM_VARS (21) + `"k:v,k:v"`; M5 with GPS: `"gps:0"` or `"gps:1"` | M:1853-1866 |
| 41 | SET_CUSTOM_VAR | len ≥ 4; `"name:value"` | OK / ERR 6; `gps` and `gps_interval` saved | M:1867-1892 |
| 42 | GET_ADVERT_PATH | len ≥ 34; `[1]` reserved, key (compared on 7 bytes) | ADVERT_PATH (22): received ts u32, path_len, path / ERR 2 | M:1893-1913 |
| 43 | GET_TUNING_PARAMS | — | TUNING_PARAMS (23): rx_delay ×1000, airtime_factor ×1000 | M:1481-1487 |
| 44-49 | (WiFi, reserved) | | ERR 1 | M:52 |
| 50 | SEND_BINARY_REQ | len ≥ 34; key, data | SENT (tag); result 0x8C | M:1720-1740 |
| 51 | FACTORY_RESET | `[1..5]` = `"reset"` (compared whatever the length) | shuts down all interfaces, erases storage and bonds, OK (never received), reboots after 1 s | M:1965-1977 |
| 52 | SEND_PATH_DISCOVERY_REQ | `[1]` = 0 and len ≥ 34; key `[2..33]` | telemetry request forced to flood; SENT; result 0x8D; the contact's path is **not** updated | M:1651-1679, 779-808 |
| 54 | SET_FLOOD_SCOPE_KEY (v8+) | `[1]` = 0: `[2..17]` key (len ≥ 18) sets the scope, shorter clears it; `[1]` = 1 (v12+): unscoped | OK; **RAM only** | M:1978-1988 |
| 55 | SEND_CONTROL_DATA (v8+) | len ≥ 2; payload `[1..]` with `[1] & 0x80` | zero-hop CONTROL; OK / ERR 3 | M:2015-2022 |
| 56 | GET_STATS (v8+) | len ≥ 2; `[1]` type 0/1/2 | STATS (24) + type: 0 = mV u16, uptime s u32, err_flags u16, queue u8; 1 = noise i16, RSSI i8, SNR×4 i8, TX airtime s u32, RX s u32; 2 = seven u32 counters (received, sent, sent flood, sent direct, received flood, received direct, errors). Other → ERR 6 | M:1914-1964 |
| 57 | SEND_ANON_REQ | len > 33; key, data | v13+: unknown key → temporary contact (type 0, zero-hop path). SENT; result 0x8C; full → ERR 3 | M:1599-1629 |
| 58 | SET_AUTOADD_CONFIG | `[1]` mask (read without length check), `[2]` optional max_hops (if len ≥ 3), capped at 64 without error | saved; OK | M:2023-2029 |
| 59 | GET_AUTOADD_CONFIG | — | AUTOADD_CONFIG (25): mask, max_hops | M:2030-2035 |
| 60 | GET_ALLOWED_REPEAT_FREQ | — | (26) + pairs (low u32, high u32) in kHz: {433000,433000}, {869495,869495}, {918000,918000} | M:1032-1040, 2036-2044 |
| 61 | SET_PATH_HASH_MODE | `[1]` = 0 and len ≥ 3 (otherwise ERR 1), `[2]` mode 0..2 | saved; OK / ERR 6 | M:1504-1511 |
| 62 | SEND_CHANNEL_DATA | len ≥ 4 (otherwise **ERR 6**); channel, path_len (0xFF = flood), path, data_type u16, payload (the rest); path and data_type read without length check | OK. ERR 6 invalid `path_len` (before anything else); then ERR 2 channel; ERR 6 data_type 0 or payload > 165 (`MAX_GROUP_DATA_LENGTH` = 184 − 16 − 3, `src/MeshCore.h:21`); ERR 3 failure | M:1198-1237 |
| 63 | SET_DEFAULT_FLOOD_SCOPE | len ≥ 48: name `[1..31]` (NUL, 1..30 chars), key `[32..47]`; shorter clears | saved; OK / ERR 6 | M:1989-2005 |
| 64 | GET_DEFAULT_FLOOD_SCOPE | — | (28) + name[31] + key[16] (48 bytes), or 1 byte if none | M:2006-2014 |
| 65 | SEND_RAW_PACKET | len ≥ 4; `[1]` priority, raw packet | transmitted as is; OK / ERR 6 / ERR 3 | M:2045-2058 |
| 66 | RUN_CLI_COMMAND (v14+) | len ≥ 3; ASCII command | CLI_REPLY (29) + text; an `"xx|"` prefix is echoed back. Commands: `src/helpers/CommonRadioPrefs.cpp:31-231`, M:2079-2211 | M:1118-1131 |

## 3. Response formats

### 3.1 SELF_INFO (5) and DEVICE_INFO (13)

SELF_INFO, 89 bytes at most (M:1085-1117):

| Position | Field |
|---|---|
| [1] | 1 (chat type) |
| [2] | TX power (dBm) |
| [3] | max power (22) |
| [4..35] | public key |
| [36..43] | lat, lon (i32 ×1e6) |
| [44] | multi_acks |
| [45] | advert_loc_policy |
| [46] | telemetry: env << 4 \| pos << 2 \| base |
| [47] | manual_add_contacts |
| [48..51] | frequency kHz, `(uint32)(freq_float × 1000)` in C++; us: exact value (deliberate deviation 2, no effect in the usual bands: the real M5 returns 869618) |
| [52..55] | BW in Hz |
| [56] | SF |
| [57] | CR |
| [58..] | name, no NUL |

DEVICE_INFO, 82 bytes (M:1059-1075):

| Position | Field |
|---|---|
| [1] | 14 |
| [2] | max contacts / 2 (175) |
| [3] | max channels (40) |
| [4..7] | `prefs.ble_pin` (0 when the PIN is random) |
| [8..19] | build date, NUL-padded (12) |
| [20..59] | manufacturer, zero-padded (40) |
| [60..79] | version, zero-padded (20) |
| [80] | repeat enabled |
| [81] | path_hash_mode |

For oxymesh: manufacturer `"Elecrow ThinkNode M5"`, version `"v1.18.0-oxymesh"` (20 bytes at most), actual build date in `"dd-Mmm-yyyy"` format.

### 3.2 Contact (CONTACT 3 and NEW_ADVERT 0x8A notification), 148 bytes (M:171-192)

| Position | Field |
|---|---|
| [1..32] | public key |
| [33] | type (0 none, 1 chat, 2 repeater, 3 room server, 4 sensor) |
| [34] | flags (bit 0 favorite, 1 base telemetry, 2 location, 3 environment, 4 remote CLI) |
| [35] | out_path_len |
| [36..99] | out_path (64 bytes) |
| [100..131] | name (32, NUL-padded) |
| [132..135] | last advert ts (sender's clock) |
| [136..143] | lat, lon |
| [144..147] | lastmod (our clock) |

### 3.3 Raw advert packet (EXPORT, IMPORT, stored adverts)

`header ‖ path_len ‖ path ‖ payload` (`src/Packet.cpp:52-63`). Self export: header 0x11 (FLOOD, ADVERT), path_len 0 (M:1389). Stored adverts keep the received path, route forced to FLOOD, transport codes removed (BCM:141-149).

## 4. Notifications (≥ 0x80)

Unless stated otherwise, a notification is only sent if an interface is connected; otherwise it is **lost**. Only messages survive, through the offline queue.

| Code | Name | Format | When | Source |
|---|---|---|---|---|
| 0x80 | ADVERT | key (32) | advert from a known contact, or from a contact just auto-added | BCM:156, 200; M:358-367 |
| 0x81 | PATH_UPDATED | key (32) | path learned from a PATH | M:409-415 |
| 0x82 | SEND_CONFIRMED | ACK (4), round trip ms u32; 9 bytes | ACK recognized in the ring (the entry is cleared) | M:424-435 |
| 0x83 | MSG_WAITING | 1 byte | after each message queued | M:469-473 |
| 0x84 | RAW_DATA | SNR×4, RSSI, 0xFF, payload | direct RAW_CUSTOM received | M:832-853 |
| 0x85 | LOGIN_SUCCESS | new, 14 bytes: permissions, prefix (6), tag (4), ACL permissions, firmware level; legacy "OK", 8 bytes: 0, prefix (6) | response to a pending login | M:694-723 |
| 0x86 | LOGIN_FAIL | 0, prefix (6) | other response to a login | M:717-722 |
| 0x87 | STATUS_RESPONSE | 0, prefix (6), data | status response | M:724-738 |
| 0x88 | LOG_RX_DATA | SNR×4 (i8), RSSI (i8), raw packet | every received packet, if connected and `len + 3 ≤ 176` | M:291-302 |
| 0x89 | TRACE_DATA | 0, path_len, flags, tag, auth, hashes (path_len bytes), SNR × 4 per hop (`path_len >> (flags & 3)` bytes), final SNR | trace that reached the end | M:855-886 |
| 0x8A | NEW_ADVERT | full contact (§3.2) | advert from a contact **not** added (manual mode, excluded type, too many hops, table full) | BCM:158-182 |
| 0x8B | TELEMETRY_RESPONSE | 0, prefix (6), CayenneLPP | telemetry response (and own telemetry) | M:739-749 |
| 0x8C | BINARY_RESPONSE | 0, tag (4), data | response to BINARY_REQ or ANON_REQ, and any unhandled response | M:750-776 |
| 0x8D | PATH_DISCOVERY_RESPONSE | 0, prefix (6), out_path_len, out_path, in_path_len, in_path | path discovery response | M:779-804 |
| 0x8E | CONTROL_DATA | SNR×4, RSSI, path_len, payload | zero-hop CONTROL with 0x80 | M:810-830 |
| 0x8F | CONTACT_DELETED | key (32) | oldest contact overwritten (table full) | M:342-349 |
| 0x90 | CONTACTS_FULL | 1 byte | no room for an advert | M:351-356 |

Only one request (login, status, telemetry, discovery, binary) pending at a time: each new one cancels the others (M.h:192-194).

## 5. Received messages and offline queue

**Offline queue** (M:224-260): FIFO of `OFFLINE_QUEUE_SIZE` frames (direct messages 7/16, channels 8/17, channel data 27): 16 by default (M.h:61-62), 256 in every ThinkNode M5 companion build (`variants/thinknode_m5/platformio.ini`), so 256 here. The Heltec V4 USB build used as the reference keeps the default 16. Full: the oldest channel frame is removed; if there is none, the new one is dropped. MSG_WAITING is sent anyway. **The format is fixed at queueing time**, according to `app_target_ver` at that moment. (**Deliberate deviation 1**: our queue survives a reboot.)

**CONTACT_MSG_RECV_V3 (16)** (M:440-467): SNR×4, 0, 0, sender prefix (6), path_len (the packet's if received by flood, **0xFF if direct**), txt_type, ts (4), author prefix (4, only for txt_type 2), text. **Legacy (7)**: the same without `[1..3]`.

**CHANNEL_MSG_RECV_V3 (17)** (M:558-583): SNR×4, 0, 0, channel, path_len (the packet's if received by flood, **0xFF if direct**, M:572), txt_type (0), ts (4), `"Name: text"`. **Legacy (8)**: the same without `[1..3]`.

**CHANNEL_DATA_RECV (27)** (M:601-627): SNR×4, 0, 0, channel, path_len (as for 17, M:617), data_type u16, length, data.

`app_target_ver` (DEVICE_QUERY, RAM, 0 at boot): **≥ 3 → V3 frames (16/17), otherwise 7/8** (M:443, 561). Txt_type 1 received: queued. Txt_type 3 received: executed if the contact has flag 0x10, otherwise queued (M:538-548).

## 6. ACKs, retries, auto-add

- Expected ACK = `SHA256(ts ‖ attempt & 3 ‖ text ‖ our key)[0..4]` (BCM:451-471). If attempt > 3: `0x00 ‖ attempt` after the text, text limited to 158.
- **The firmware never retries** and does not report failure (`onSendTimeout` empty, M:898): the app retries with the same ts and attempt + 1, and may switch to flood through RESET_PATH. (**Deliberate deviation 6**: except for messages sent from our screen.)
- Auto-add (M:304-340, BCM:72-106, 122-201):
  - `manual_add_contacts` bit 0 at 0: every type added;
  - bit 0 at 1: only the types ticked in `autoadd_config` (0x02 chat, 0x04 repeater, 0x08 room server, 0x10 sensor);
  - `autoadd_max_hops` N > 0: refused if hops ≥ N;
  - table full: with `autoadd_config & 0x01`, the oldest (lastmod) non-favorite is overwritten (0x8F notification, stored advert erased); otherwise 0x8A then 0x90;
  - advert with ts ≤ the known one: ignored;
  - each accepted advert rewrites the stored advert and schedules the contacts write (5 s).
- Channels: channel 0 is preloaded as "Public" with PSK `izOH6cXN6mrJ5e26oRXNcg==`, then replaced by the saved data (M:1002-1003).
- Flood scope: `send_unscoped` > RAM scope (cmd 54) > default scope (saved, cmd 63) > none (M:491-525).

## 7. Default settings (M5 BLE build)

Name = first 4 bytes of the key as 8 uppercase hex digits; 869.618 MHz / 62.5 kHz / SF8 / CR 5; 22 dBm; airtime_factor 1.0; tx_delay 0.5; direct 0.2; rx_delay 0; multi_acks 0; manual_add 0; telemetry 0/0/0; advert_loc_policy 0; ble_pin 0; repeat disabled; path_hash_mode 0; autoadd 0/0; boosted RX gain; GPS disabled; no default scope; no advert at boot (M:916-1007).

## 8. Persistence

### 8.1 C++ firmware (to read for migration, ADR-0006)

SPIFFS (`SPIFFS.begin(true)`), default arduino-esp32 table: `spiffs` at 0x290000, 0x160000 (to be confirmed on the device).

| File | Format | Source |
|---|---|---|
| `/identity/_main.id` | 96 bytes: pub(32) ‖ prv(64) orlp expanded | `src/Identity.cpp:111-121`, `src/helpers/IdentityStore.cpp:45-65` |
| `/prefs.json` | ConfigSerializer pseudo-JSON (unquoted keys, floats with 4 decimals, lat/lon with 6, binary as uppercase hex): `{name, lat, lon, radio:{freq,bw,sf,cr,cad,int_thr,rxgain,fem_rxgain,fem_txgain,tx,af,rxdelay,f_txdelay,d_txdelay,agc_int,hash_mode,multi_ack}, gps:{en,int,adv_loc}, repeat:{disable}, comp:{auto_max,defs_nm,defs_key,pin,buzz_q,vibe_q,auto_add,man_add,tel_base,tel_loc,tel_env,tz_offset}, custom:{…}}` | NP:62-207, `src/helpers/ConfigSerializer.cpp` |
| `/new_prefs` | legacy binary, read if `prefs.json` is missing | DS:212-240 |
| `/contacts3` | 152-byte records: pub 32, name 32, type 1, flags 1, unused 1, sync_since 4, out_path_len 1, last_advert 4, out_path 64, lastmod 4, lat 4, lon 4 | DS:259-321 |
| `/channels2` | 40 records of 68 bytes: 4 zeros, name 32, secret 32; index = channel number | DS:323-366 |
| `/bl/<16 uppercase hex digits of the first 8 key bytes>` | raw advert packet ≤ 255 bytes | DS:559-603 |

### 8.2 oxymesh

`mcdata` partition (sequential-storage, ADR-0006): identity, settings (versioned binary), contacts, channels, stored adverts, offline queue, local screen history. The format is defined in MC4 in this document.

## 9. Deliberate deviations (ADR-0001)

1. Offline queue kept in flash.
2. Exact frequency in SELF_INFO (identical to the C++ in 433-435, 863-870 and 902-928 MHz, checked for every kHz; 1 kHz less on the C++ side for about 1 % of frequencies elsewhere).
3. Channel name zero-padded in CHANNEL_INFO.
4. No truncation of 174-176-byte frames over Bluetooth (MTU ≥ 179).
5. *(Withdrawn in MC5: the PIN is drawn at boot as in the C++, now that the Bluetooth host takes a fixed passkey.)*
6. Automatic retries for messages sent from the screen.
7. SEND_CHANNEL_TXT_MSG shorter than 7 bytes: ERR 1. The C++ does not check the length there and passes a negative text length to `memcpy` (M:1178-1197, BCM:528): likely crash, except for type ≠ 0 (ERR 1) or an invalid channel (ERR 2).
8. Bytes read beyond the frame (§10): zeros. The C++ reads the rest of `cmd_frame` (M.h:267), a buffer of the global object `the_mesh` (`main.cpp:117`, hence zero at boot) that keeps the bytes of previous frames (only `cmd_frame[len]` is sometimes reset to 0): it reads zeros as long as no longer frame has gone through since boot. No known app sends such frames.
9. A new PIN (`SET_DEVICE_PIN`, or from the screen) applies at once instead of at the next boot; back to 0 draws a new random PIN. The app does not see the active PIN either way.
10. USB stays a companion interface next to Bluetooth (the M5's C++ BLE build has Bluetooth only). As in §1.3, every frame goes out on both and either can send commands. With nothing on the USB port this is invisible.
11. The GAP device name (read after connecting) holds at most 22 bytes, the `trouble-host` limit: `MeshCore-` and the first 13 bytes of the node name. The advertised name, which apps show while scanning, is whole up to 29 bytes as in the C++.

## 10. Device-side decoding (ADR-0002)

`Command::decode_device(frame) -> Result<Command, DeviceReject>` reproduces the parsing of `MyMesh::handleCmdFrame` (M:1055-2062), branch by branch:

- failed **precondition** → `DeviceReject::Err(1)` (last `else`, M:2059), except for the three branches that answer `ERR 6` (§0);
- **optional trailing fields** depending on length → `Option`; encoding a `None` omits the field;
- **extra bytes** ignored wherever the C++ ignores them;
- **bytes read beyond the frame** (the C++ does not always check the length): 0 (deliberate deviation 8);
- empty frame or frame over 176 bytes → `DeviceReject::Ignored`: the interfaces never hand it to the C++ (§1), no response.

**Not** the decoder's job: the checks the C++ makes after parsing or after looking up state (radio bounds, PIN, power, lat/lon, stats type, hash mode ≥ 3, `data_type` 0, scope name length, `"name:value"`, unknown contact or channel, invalid raw packet…). The state machine (`mc-node`) makes them, in the C++ order.

**Invariant** (checked by the `[[device]]` vectors, proptest and the `cmd_decode` fuzz target): every command accepted by `decode_device` is written by `encode` into a canonical frame that strict decoding reads back identically; every frame accepted in strict mode also passes `decode_device`, with the same meaning.

| Code | Precondition (otherwise ERR 1) | Optional fields, extra bytes | Read outside the frame (0) | Source |
|---|---|---|---|---|
| 1 APP_START | len ≥ 8 | name = `[8..]` | — | M:1076 |
| 2 SEND_TXT_MSG | len ≥ 14 | text = `[13..]` | — | M:1132 |
| 3 SEND_CHANNEL_TXT_MSG | none; us: len ≥ 7 (deviation 7) | text = `[7..]`, may be empty | — | M:1178, BCM:528 |
| 4 GET_CONTACTS | none | since if len ≥ 5 (2 to 4 bytes: ignored); extra ignored | — | M:1242-1246 |
| 5, 10, 20, 23, 33, 35, 40, 43, 59, 60, 64 | none | extra ignored | — | M:1281, 1415, 1517, 1528, 1781, 1801, 1853, 1481, 2030, 2036, 2006 |
| 6 SET_DEVICE_TIME | len ≥ 5 | extra ignored | — | M:1287 |
| 7 SEND_SELF_ADVERT | none | `[1]` optional: 1 = flood, any other value = zero-hop; extra ignored | — | M:1305 |
| 8 SET_ADVERT_NAME | len ≥ 2 | name = `[1..]` | — | M:1259 |
| 9 ADD_UPDATE_CONTACT | len ≥ 36 | lat/lon if len ≥ 144, lastmod if len ≥ 148; extra ignored | `[36..135]` | M:1328, 194-217 |
| 11 SET_RADIO_PARAMS | none | repeat if len ≥ 12; extra ignored | `[1..10]` | M:1424-1436 |
| 12 SET_RADIO_TX_POWER | none | extra ignored | `[1]` | M:1460 |
| 13 RESET_PATH, 27 SEND_STATUS_REQ, 28 HAS_CONNECTION, 29 LOGOUT | len ≥ 33 | extra ignored | — | M:1317, 1630, 1741, 1748 |
| 14 SET_ADVERT_LATLON | len ≥ 9 | alt if len ≥ 13 (10 to 12: ignored); extra ignored | — | M:1266-1272 |
| 15 REMOVE_CONTACT, 16 SHARE_CONTACT, 30 GET_CONTACT_BY_KEY | none | extra ignored | key `[1..32]` | M:1349, 1359, 1371 |
| 17 EXPORT_CONTACT | none | len < 33: self (bytes ignored); extra ignored | — | M:1380 |
| 18 IMPORT_CONTACT | len > 98 | packet = `[1..]` | — | M:1409 |
| 19 REBOOT | `[1..6]` = `"reboot"` | extra ignored | `[1..6]` | M:1512 |
| 21 SET_TUNING_PARAMS | none | extra ignored | `[1..8]` | M:1470 |
| 22 DEVICE_QUERY | len ≥ 2 | extra ignored | — | M:1056 |
| 24 IMPORT_PRIVATE_KEY | len ≥ 65 | extra ignored | — | M:1537 |
| 25 SEND_RAW_DATA | len ≥ 6; valid `path_len` (0xFF included → ERR 1); payload ≥ 4 bytes after the path (otherwise **ERR 6**) | payload = the rest | — | M:1557-1576 |
| 26 SEND_LOGIN | len ≥ 33 | password = `[33..]`, may be empty | — | M:1577 |
| 31 GET_CHANNEL | len ≥ 2 | extra ignored | — | M:1752 |
| 32 SET_CHANNEL | 50 ≤ len ≤ 65 (len ≥ 66: 32-byte key, ERR 1) | `[50..]` ignored | — | M:1767-1769 |
| 34 SIGN_DATA | len ≥ 2 | chunk = `[1..]` | — | M:1793 |
| 36 SEND_TRACE_PATH | len ≥ 11; hashes consistent with `flags` (otherwise **ERR 6**) | hashes = `[10..]` | — | M:1813-1818 |
| 37 SET_DEVICE_PIN | len ≥ 5 | extra ignored | — | M:1839 |
| 38 SET_OTHER_PARAMS | none | `[2]`, `[3]`, `[4]` if len ≥ 3, 4, 5; extra ignored | `[1]` | M:1488-1501 |
| 39 SEND_TELEMETRY_REQ | len ≥ 36 (key `[4..35]`) or len = 4 (self) | `[1..3]` reserved, kept; extra ignored (len ≥ 36) | — | M:1680, 1700 |
| 41 SET_CUSTOM_VAR | len ≥ 4 | text = `[1..]` | — | M:1867 |
| 42 GET_ADVERT_PATH | len ≥ 34 | `[1]` reserved, kept; extra ignored | — | M:1893 |
| 50 SEND_BINARY_REQ, 57 SEND_ANON_REQ | len ≥ 34 | data = `[33..]` | — | M:1720, 1599 |
| 51 FACTORY_RESET | `[1..5]` = `"reset"` | extra ignored | `[1..5]` | M:1965 |
| 52 SEND_PATH_DISCOVERY_REQ | `[1]` = 0 and len ≥ 34 | extra ignored | — | M:1651 |
| 54 SET_FLOOD_SCOPE_KEY | len ≥ 2 and `[1]` ∈ {0, 1} | `[1]` = 0: key if len ≥ 18, otherwise clear (bytes ignored); extra ignored | — | M:1978-1988 |
| 55 SEND_CONTROL_DATA | len ≥ 2 and `[1] & 0x80` | payload = `[1..]` | — | M:2015 |
| 56 GET_STATS | len ≥ 2 | extra ignored | — | M:1914 |
| 58 SET_AUTOADD_CONFIG | none | max_hops if len ≥ 3; extra ignored | `[1]` | M:2023-2027 |
| 61 SET_PATH_HASH_MODE | `[1]` = 0 and len ≥ 3 | extra ignored | — | M:1504 |
| 62 SEND_CHANNEL_DATA | len ≥ 4 and `path_len` valid or 0xFF (otherwise **ERR 6**) | payload = what remains after `data_type` (empty if nothing remains) | path and `data_type` | M:1198-1223 |
| 63 SET_DEFAULT_FLOOD_SCOPE | none | scope if len ≥ 48, otherwise clear (bytes ignored); extra ignored | — | M:1989-2005 |
| 65 SEND_RAW_PACKET | len ≥ 4 | packet = `[2..]` | — | M:2045 |
| 66 RUN_CLI_COMMAND | len ≥ 3 | command = `[1..]` | — | M:1118 |
| 0, 44-49, 53, 67-255 | — | — | — | ERR 1, M:2059 |

SEND_CHANNEL_DATA whose path overflows the frame: `data_type` is then read outside the frame, hence 0, and the command always fails (ERR 2 if the channel is invalid, otherwise ERR 6) whatever the path bytes; `decode_device` returns a zero path.
