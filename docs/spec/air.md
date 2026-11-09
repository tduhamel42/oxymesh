# MeshCore over-the-air packets (v1.18)

Source of truth for `mc-air` and the mesh layer of `mc-node`. Reference: MeshCore `companion-v1.18.0`, commit 81dbae5 (ADR-0001). `file:line` citations refer to this commit. Integers are **little-endian** unless stated otherwise.

Abbreviations: `rpt` = `examples/simple_repeater/MyMesh.cpp`, `comp` = `examples/companion_radio/MyMesh.cpp`, `room` = `examples/simple_room_server/MyMesh.cpp`, `BCM` = `src/helpers/BaseChatMesh.cpp`.

## 1. Packet

```
header(1) | [tc0 u16 | tc1 u16]  (routes 0 and 3 only) | path_len(1) | path | payload (the rest)
```

- No length field: the payload is whatever remains of the LoRa frame.
- Packet ≤ 255 bytes (`MAX_TRANS_UNIT`, `src/MeshCore.h:23`); payload ≤ 184 (`MAX_PACKET_PAYLOAD`, `:20`); path ≤ 64 bytes (`MAX_PATH_SIZE`, `:22`).
- Reception (`Dispatcher::tryParsePacket`, `src/Dispatcher.cpp:149-189`):
  - header version > 0 → reject;
  - hash size encoded as 3 (4 bytes) → reject;
  - path bytes > 64 or past the end of the packet → reject;
  - payload > 184 → reject;
  - **an empty payload is accepted** at this level; each type then enforces its own minimum length (§3).

### 1.1 Header `0bVVPPPPRR` (`src/Packet.h:8-37`)

| Bits | Field | Values |
|---|---|---|
| 0-1 | route | 0 TRANSPORT_FLOOD, 1 FLOOD, 2 DIRECT, 3 TRANSPORT_DIRECT |
| 2-5 | payload type | table below |
| 6-7 | version | 0 only (`PAYLOAD_VER_1`: 1-byte hashes, 2-byte MAC) |

| Type | Name | Type | Name |
|---|---|---|---|
| 0x0 | REQ | 0x8 | PATH |
| 0x1 | RESPONSE | 0x9 | TRACE |
| 0x2 | TXT_MSG | 0xA | MULTIPART |
| 0x3 | ACK | 0xB | CONTROL |
| 0x4 | ADVERT | 0xC-0xE | unassigned |
| 0x5 | GRP_TXT | 0xF | RAW_CUSTOM |
| 0x6 | GRP_DATA | | |
| 0x7 | ANON_REQ | | |

Header 0xFF is used internally as a "do not retransmit" mark (`Packet.h`, `markDoNotRetransmit`): never transmitted.

### 1.2 `path_len` (`src/Packet.h:79-83`, `src/Packet.cpp:9-30`)

- Bits 0-5: hop count (0-63). Bits 6-7: size of one path hash minus 1 (1, 2 or 3 bytes; 3 → 4 bytes is reserved and rejected).
- Path length = count × size, ≤ 64. Practical limits: 63 hops with 1 byte, 32 with 2, 21 with 3.
- `0xFF` = "unknown path" in contact state and in the companion protocol (`OUT_PATH_UNKNOWN`, `src/helpers/ContactInfo.h:6`); never valid over the air.
- Firmware older than 1.14 reads `path_len` as a raw byte count: a packet with 2- or 3-byte hashes looks too long to it and it drops it. By default we transmit with 1-byte hashes (`path_hash_mode` 0) and accept all three sizes.
- **Overflow to fix**: with 1-byte hashes and 63 hops, the C++ still appends one more hop and writes 0x40 (read as "2 bytes, 0 hops") (`src/Mesh.cpp:347-350`). We cap at 63 hops.

### 1.3 Transport codes (routes 0 and 3)

- `tc0` = scope code; `tc1` reserved, always transmitted as 0 (rpt:1003-1006, comp:495-498).
- `code = u16le(HMAC-SHA256(key16, [type] ‖ payload)[0..2])`; 0x0000 becomes 0x0001 and 0xFFFF becomes 0xFFFE (`src/helpers/TransportKeyStore.cpp:4-18`).
- Covers neither the path nor the header: it survives forwarding.
- Key of a region: `SHA256("#" ‖ name)[0..16]`; "fr" and "#fr" give the same key (`src/helpers/RegionMap.cpp:173-188`, `TransportKeyStore.cpp:44-47`). Private regions `$name` are not implemented in v1.18 (`TransportKeyStore.cpp:52-68`).
- `{0, 0}` in zero-hop TRANSPORT_DIRECT means "local share" (`BCM:578-580`).

### 1.4 Dedup hash (`src/Packet.cpp:41-50`)

`SHA256([type] ‖ (TRACE only: path_len as u16 LE, 2 bytes) ‖ payload)[0..8]`. The header (route, version), the path and the transport codes are not part of the hash.

## 2. Crypto

| Item | Exact rule |
|---|---|
| Ed25519 | orlp/ed25519 (`lib/ed25519`). Verification by rweather `Ed25519::verify` (`src/Identity.cpp:38`). Us: ed25519-dalek `verify` (non-strict) |
| Private key | **64-byte orlp "expanded" key**: `SHA512(seed)`, clamped scalar `[0..32]` then nonce prefix `[32..64]` (`lib/ed25519/keypair.c:6-16`). The seed is not kept. Identity file: pub(32) ‖ prv(64) (`src/Identity.cpp:111-149`) |
| Signature | Standard Ed25519 with `prv[32..64]` as nonce prefix (`lib/ed25519/sign.c:14-30`). Us: `hazmat::ExpandedSecretKey` + `raw_sign` |
| Forbidden keys | `pub[0]` ∈ {0x00, 0xFF} → key regenerated (`src/Identity.cpp:71-72`, comp:951) |
| ECDH | `x25519(clamp(prv[0..32]), u)` with `u = (1+y)/(1−y)` derived from the peer's Ed25519 `y` (sign bit ignored, point not validated) (`lib/ed25519/key_exchange.c:20-78`). 32-byte secret, computed once per contact and kept (`src/helpers/ContactInfo.h:21-27`) |
| Encryption | AES-128-**ECB**, key = `secret[0..16]`, last block zero-padded, no extra block if aligned (`src/Utils.cpp:85-125`) |
| MAC | `HMAC-SHA256(key = secret[0..32], ciphertext)[0..2]`, placed **before** the ciphertext (`src/Utils.cpp:127-145`). Reception: ciphertext not a multiple of 16 → reject; bad MAC → reject (`:147-175`) |
| Channel secret | 32-byte buffer; a 16-byte PSK is zero-padded (equivalent for HMAC). `hash = SHA256(secret[0..16 or 32])[0]`, 16 if bytes 16..32 are zero (`BCM:911-946`). Public channel: `8b3387e9c5cdea6ac9e5edbaa115cd72` (comp:114). "#name" channel: `SHA256("#name")[0..16]`, derived by the app |

Encrypted block, everywhere: `MAC(2) ‖ AES-ECB(padded plaintext)`.

## 3. Payloads

Source and destination hashes are **always 1 byte** (`PATH_HASH_SIZE`, `src/MeshCore.h:18`), whatever the path hash size. The hash of an identity is `pub[0]`.

| Type | Cleartext | Encrypted | Reception |
|---|---|---|---|
| REQ, RESPONSE, TXT_MSG, PATH | `dest(1) src(1)` | encrypted block, key = ECDH | length > 4 (`src/Mesh.cpp:139`). If `dest == self.pub[0]`, we try each contact with `pub[0] == src` (24 at most, `BCM:203-211`); the first valid MAC wins (`src/Mesh.cpp:147-185`) |
| ANON_REQ | `dest(1) sender_pub(32)` | encrypted block, key = ECDH(us, sender_pub) | length > 35 (`src/Mesh.cpp:197-224`). The key is the sender's **static** identity (the "ephemeral" comment in `Packet.h:26` is wrong) |
| GRP_TXT, GRP_DATA | `channel_hash(1)` | encrypted block, key = channel secret | tries up to 4 channels with the same hash (`src/Mesh.cpp:225-251`); unnamed channels are skipped (`BCM:401`) |
| ACK | `crc(4)` then free bytes | — | reads 4 bytes (`src/Mesh.cpp:117-129`). Lengths transmitted: 4 (servers), 6 (companions: crc, extended attempt byte, random byte, `BCM:244-256`), 5 (room server keep-alive ACK, room:571-574) |
| ADVERT | `pub(32) ts u32 sig(64) app_data(≤ 32)` | — | §5 |
| TRACE | `tag u32 auth u32 flags u8 hashes…` | — | §4.6 |
| MULTIPART | `(remaining << 4 \| inner_type)(1) content…` | — | only the ACK inner type exists; ≥ 5 bytes (`src/Mesh.cpp:300-324, 574-587`) |
| CONTROL | `flags(1) data` (subtype = high nibble) | — | not encrypted. `flags & 0x80` = zero-hop direct only, never forwarded (`src/Mesh.cpp:70-76`) |
| RAW_CUSTOM | opaque | — | only delivered when direct with 0 hops; never flood-forwarded (`src/Mesh.cpp:292-299`) |

### 3.1 Plaintexts

**TXT_MSG** (`BCM:451-471`): `ts u32 ‖ (txt_type << 2 | attempt & 3) ‖ text` (no NUL; the zero padding comes from encryption). If `attempt > 3`, `0x00 ‖ attempt u8` is appended after the text, and the text is limited to 158 bytes (160 otherwise, `MAX_TEXT_LEN`, `src/helpers/BaseChatMesh.h:8`).

| txt_type | Meaning |
|---|---|
| 0 PLAIN | text, acknowledged |
| 1 CLI_DATA | command output, no ACK |
| 2 SIGNED_PLAIN | `author_pub_prefix(4) ‖ text`, sent by room servers (room:69-91) |
| 3 CLI_COMMAND | new in v1.18 |

**REQ**: `tag/ts u32 ‖ type ‖ parameters`.
- Repeater request types (rpt:46-51): 1 GET_STATUS, 2 KEEP_ALIVE, 3 TELEMETRY, 5 GET_ACL, 6 NEIGHBOURS, 7 OWNER_INFO.
- Generic 13-byte request: `tag u32, type, 00 00 00 00, random 4` (`BCM:693-700`). Keep-alive: `ts, 0x02, sync_since u32` (`BCM:814-818`).

**RESPONSE**: `tag u32` (the request's timestamp, echoed back) `‖ body` (rpt:214).
- Login accepted, 13 bytes: `server_ts u32, 0x00, 0x00 (legacy), is_admin, permissions, random 4, firmware_level = 2` (rpt:135-144, room:369-376).
- Legacy login response: `"OK"` at position 4 (comp:699-703).

**PATH** (`src/Mesh.cpp:443-486`, decoding `:160-172`): `path_len (encoded) ‖ path ‖ extra_type ‖ extra`.
- Only the low nibble of `extra_type` is read.
- Without extra: `0xFF` then 4 random bytes (makes the hash unique).
- Extra = ACK (4 or 6 bytes) or RESPONSE.
- Refused at creation if `path_bytes + extra_len + 5 > 166`.

**ANON_REQ** (`BCM:603-661`, rpt:582-592):
- login to a repeater or sensor: `ts ‖ password (≤ 15 bytes)`;
- login to a room server: `ts ‖ sync_since u32 ‖ password`;
- anonymous requests to a repeater (only accepted when they arrive direct): `ts ‖ type (1 regions, 2 owner, 3 clock) ‖ reply_path_len ‖ reply_path`; the repeater reads `data[4] == 0 || data[4] ≥ 0x20` as a password.

**GRP_TXT**: `ts u32 ‖ 0x00 ‖ "name: text"`; rejected if `(pt[4] >> 2) != 0` (`BCM:410-430, 518-537`).

**GRP_DATA**: `data_type u16 ‖ length u8 ‖ data` (`BCM:431-448, 539-555`); data ≤ 165 bytes (`MAX_GROUP_DATA_LENGTH`).

**DISCOVER** (CONTROL, rpt:786-860):
- request: `0x80 | prefix_only, type_filter (1 << ADV_TYPE), tag u32, [since u32]`;
- response: `0x90 | node_type, snr×4 (i8), tag u32, public key (8 or 32 bytes)`, sent zero-hop after `repeater_delay × 4`; at most 4 responses per 120 s.

### 3.2 ACKs (ACK CRC)

`SHA256(ts u32 ‖ flags ‖ text without NUL ‖ SENDER's pub)[0..4]`, kept as a little-endian u32 and transmitted as is.
- The sender computes it with its own key (`BCM:462`), the recipient with the sender's key (`BCM:245`).
- The extended attempt tail (`0x00 ‖ attempt`) is not included; only `attempt & 3` counts.
- SIGNED_PLAIN: `SHA256(pt[0..9+strlen] ‖ RECIPIENT's pub)` (`BCM:298`, room:88).
- Keep-alive: `SHA256(9-byte request ‖ client's pub)` (`BCM:821`, room:569).

## 4. Routing

### 4.1 Reception (`src/Mesh.cpp:41-332`), in order

1. TRACE.
2. CONTROL with the 0x80 bit.
3. Direct with a non-empty path: forwarding only, with ACKs snooped on the fly (`:78-110`).
4. Flood: `filterRecvFloodPacket` filter.
5. Dispatch by type. An unknown type is not forwarded (`:326-329`).

### 4.2 Direct forwarding

- If `path[0..size] == self.pub[0..size]` and `allow_forward`: the first entry is removed (shift, `:334-342`) and the packet is requeued at priority 0 after the direct delay.
- ACKs and MULTIPART-ACKs are not forwarded as is: new ACKs are built, plus `multi_acks` MULTIPART copies spaced 300 ms apart (`:91-99, 359-402`).
- The recipient receives the packet with an empty path and processes it normally.

### 4.3 Flood forwarding (`routeRecvPacket`, `:344-357`)

- Conditions: not marked "do not retransmit", `(n+1) × size ≤ 64`, `allow_forward`.
- `self.pub[0..size]` is appended and the packet is queued at priority = new hop count, after the flood delay.
- A packet that decrypts for us is marked "do not retransmit" (`:188, 218`; `BCM:376`).
- An advert is only forwarded if its signature is valid (`:282-288`).

### 4.4 Dedup (`src/helpers/SimpleMeshTables.h:9-57`)

Ring of 160 8-byte hashes, checked **before** decryption. Our own transmissions are recorded in it (sendFlood, sendDirect, sendZeroHop).

### 4.5 Delays, queues, duty cycle

| Who | Flood delay | Direct delay |
|---|---|---|
| companion, repeater, room server | `rand[0, 5·t]` with `t = airtime(path_bytes + payload + 2) × f_txdelay` (0.5), computed after appending ourselves | same formula with `d_txdelay` = 0.2 (companion, room server) or 0.3 (repeater), computed before removing ourselves (comp:278-285, rpt:547-554) |

- `nextInt(a, b) = rand32 % (b − a) + a` (`src/Utils.cpp:18-22`).
- Score-based receive delay: disabled everywhere (`rx_delay_base = 0`, comp:273-276).
- Priorities (lowest first among due packets, then arrival order; `src/helpers/StaticPoolPacketManager.cpp:22-44`, `src/Mesh.cpp:653-725`):

| Packet | Priority |
|---|---|
| sendDirect | 0 (PATH: 1, TRACE: 5) |
| sendZeroHop | 0 |
| our own sendFlood | 1 (PATH: 2, ADVERT: 3) |
| forwarded flood | hop count |
| forwarded TRACE | 5 |

- Duty cycle (`src/Dispatcher.cpp:19-53, 92-106, 275-286`): budget = 1 h window × 1/(1 + af) (af = 1 → 50 %). A transmission requires a budget ≥ airtime(255)/2; below 100 ms of budget, we wait. Transmit guard timeout: 1.5 × airtime (`:327-339`). **On our side, the EU limits from `config/eu868.toml` apply on top** (`Node::apply_regulatory`): when the occupied band lies within one of the profile's sub-bands, the most permissive rule that admits the radiated power applies, its duty cycle kept by a bucketed sliding-hour budget with a 1 % margin (g3: 10 %, g2: 0.1 %), and the SX1262 power is lowered until the radiated power (setting + the board's front-end gain, e.g. +7 to +11 dB on the Heltec V4) fits the sub-band's limit (deviation 12 in `companion.md` §9). Outside the profile (other regions, a band straddling two sub-bands), only the C++ budget applies.
- Listen before talk (`src/Dispatcher.cpp:289-305`), asked only when a frame is ready: the channel is busy if a packet is coming in, if RSSI > floor + `interference_threshold` (0 = disabled by default), or on hardware CAD (disabled by default; reception restarts after it) (`RadioLibWrappers.cpp:198-214`). Busy: retry after 120, 240 or 360 ms; after 4 s, transmit anyway (`ERR_EVENT_CAD_TIMEOUT`).
- "A packet is coming in" (`CustomSX1262::isReceiving`, `CustomSX1262.h:108-149`; `mc_core::rx_watch`): HEADER_VALID for at most the airtime of a 255-byte payload, or PREAMBLE_DETECTED alone for at most the preamble and header time (`calcMaxPacketMillis`, `RadioLibWrappers.cpp:244-260`: about 182 ms and 2.1 s at SF8 / 62.5 kHz / CR 4/8). Past that, or on HEADER_ERR, the latched flags are cleared (0x0004 for a preamble alone, 0x003C otherwise) and the channel reads free again. The noise floor sampler uses the same answer.
- AGC reset (`agc_reset_interval`, in 4 s units, 0 = off by default; `Dispatcher.cpp:127-136`, `RadioLibWrappers.cpp:74-88`, `SX126xReset.h`): due at once, then every interval, counted from the end of the last transmission; skipped when a packet is coming in or waiting. The radio goes to warm sleep, then standby, full calibration (0x89 0x7F), the RF switch again, then its settings (image calibration for the frequency, RX gain) and reception; the noise floor starts over from 0.

### 4.6 Who forwards (`allow_forward`)

| Node | Default | Rules |
|---|---|---|
| Companion | **no** (`repeat.disable_fwd = 1`) | if enabled: no hop or region limit; can only be enabled on 433.000, 869.495 or 918.000 MHz (comp:487-489, 1032-1040, 1439-1447) |
| Repeater | yes | limits `flood_max` 64, `flood_max_unscoped` 64, `flood_max_advert` 8 (hops before appending); region; loop detection (rpt:396-459, `src/helpers/RoutingPolicy.h:15-22`) |
| Room server, sensor | no | |

Per-region scope (repeaters, rpt:556-569): TRANSPORT_FLOOD must match a region without `DENY_FLOOD`; plain FLOOD only if the `*` region allows it; direct is never filtered. None of this affects local delivery.

### 4.7 Path learning

1. A REQ, TXT or ANON_REQ reaches B by flood. B floods (within scope) a PATH containing the arrival path, with the ACK or the response as extra (`BCM:250-254, 316-320`).
2. A keeps this path as the contact's `out_path`.
3. A companion then sends a reciprocal PATH back **direct** along this path, after 500 ms (`src/Mesh.cpp:173-178`); servers do not (rpt:782-783).
4. If an ACK or a RESPONSE arrives by flood while a direct path is known, a PATH is sent back direct after 3 s (`BCM:378-390`).

### 4.8 TRACE (`src/Mesh.cpp:42-68, 605-619, 698-704`)

- The initiator transmits direct, `path_len = 0`, with the list of hashes in the payload. Flood TRACE is refused (`:638-641`).
- At each hop: `off = path_len << (flags & 3)`. If `off ≥ hashes length`, the trace is complete (`onTraceRecv`). Otherwise, if `hashes[off .. off + (1 << sz)]` is ours, we may forward and the packet is new: `i8(snr × 4)` is appended to the header's path field, `path_len` is incremented, priority 5.
- Trace hashes are 1, 2, 4 or 8 bytes (not 1/2/3).

## 5. Advert (ADVERT)

- Signed bytes: `pub(32) ‖ ts u32 ‖ app_data` (`src/Mesh.cpp:404-438`). On reception, `app_data` is truncated to 32 bytes **before** verification (`:269-281`).
- `app_data` (`src/helpers/AdvertDataHelpers.h:7-17`, `.cpp:4-69`), in order:

| Field | Present | Format |
|---|---|---|
| flags | always | low nibble = type (0 none, 1 chat, 2 repeater, 3 room server, 4 sensor); 0x10 position; 0x20 feat1; 0x40 feat2; 0x80 name |
| lat, lon | 0x10 | i32 each, degrees × 1e6 truncated |
| feat1, feat2 | 0x20, 0x40 | u16 each |
| name | 0x80 | the rest, no NUL, cut on a UTF-8 boundary |

- A companion ignores an advert without a name (`BCM:124-127`).
- `ts` = RTC time in Unix seconds (`src/Mesh.cpp:418`). Anti-replay: a companion rejects `ts ≤ last ts` for a known contact (`BCM:133-136`).
- The companion does not advertise at boot; it advertises on request from the app (`SEND_SELF_ADVERT`), zero-hop or flooded within the default scope.

## 6. Radio

| Parameter | Value | Source |
|---|---|---|
| Build default frequency / BW / SF / CR | 869.618 MHz / 62.5 kHz / SF8 / CR 4/5 | `platformio.ini:29-31`, `src/helpers/radiolib/CustomSX1262.h:27-31` |
| France region (app preset, meshcore.fr) | 869.618 / 62.5 / SF8 / **CR 4/8** | `config/regions.toml` |
| Sync word | RadioLib "private" (0x12, SX126x registers 0x0740-0x0741 = 0x14, 0x24) | `CustomSX1262.h:56` |
| Preamble | 32 symbols if SF ≤ 8, otherwise 16 | `RadioLibWrappers.h:56`, `CustomSX1262Wrapper.h:20` |
| CRC | enabled | `CustomSX1262.h:69` |
| Header, IQ, LDRO | RadioLib defaults: explicit, normal IQ, automatic LDRO (symbol ≥ 16 ms) | to be confirmed against the Heltec in MC1 |
| Boosted RX gain | enabled (`rx_boosted_gain`, `radio.rxgain` in the console) | comp:936-941 |
| Image calibration | the SX126x table band around the frequency (datasheet §9.2.1); ±4 MHz outside the table | RadioLib `calibrateImage` |
| SNR | packet SNR × 4, as i8 | `src/Dispatcher.cpp:206` |
| Noise floor | average of 64 instantaneous RSSI readings while idle, keeping those < floor + 14, floor ≥ −120; recalibrated every 2 s | `RadioLibWrappers.cpp:90-110`, `src/Dispatcher.cpp:15-17, 67-71` |

## 7. Compatibility with older versions

- Header version ≠ 0: reject.
- 2- or 3-byte path hashes: accepted, never transmitted by default (§1.2).
- TRACE `flags & 3` (hash size): since v1.11.
- TXT type 3: since v1.18.
- ACK: accept ≥ 4 bytes, read only the first 4.
- Legacy "OK" login response and keep-alive (REQ 2, obsolete but still served by room servers).
- CLI command as plain text with ACK (legacy); servers now answer with CLI_DATA (rpt:717-731, 749).

## 8. Vectors and oracle

- `testdata/air/gen.py`: **independent** oracle, in pure Python (AES FIPS-197, SHA-256 and HMAC from `hashlib`/`hmac`, X25519 RFC 7748, Ed25519 RFC 8032 with expanded keys). It produces `testdata/air/vectors.toml`.
- Captures from **our** devices (test identities imported into the Heltec with `IMPORT_PRIVATE_KEY`): adverts, DMs, Public channel, datagrams.
- `air_decode` fuzz target: never panics; every accepted packet re-encodes identically.
