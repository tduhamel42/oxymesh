// MeshCore companion protocol vectors (docs/spec/companion.md), written byte by byte from the
// spec (taken from loranet, testdata/meshcore).
// Independent oracle: every frame that meshcore.js can produce or read is cross-checked with it
// (identical encoding for commands, identical decoded fields for responses). Frames it does not
// know are written from the spec alone, marked "spec only".
//
// [[device]] section: what MyMesh::handleCmdFrame accepts (docs/spec/companion.md §10), written by
// hand from the C++ (line in the name): expect = "ok" (with the strict canonical form),
// "err:N" (ERR N response) or "ignored" (frame the carriers never hand over).
//
// Usage: (cd testdata/proto && npm ci && node gen.mjs vectors.toml)
import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import assert from "node:assert/strict";
import { Connection, Constants } from "@liamcottle/meshcore.js";

const hex = (b) => Buffer.from(b).toString("hex");
const cat = (...parts) => Buffer.concat(parts.map((p) => (typeof p === "number" ? Buffer.from([p]) : Buffer.from(p))));
const u16 = (v) => { const b = Buffer.alloc(2); b.writeUInt16LE(v); return b; };
const u32 = (v) => { const b = Buffer.alloc(4); b.writeUInt32LE(v >>> 0); return b; };
const i32 = (v) => { const b = Buffer.alloc(4); b.writeInt32LE(v); return b; };
const fixed = (s, n) => { const b = Buffer.alloc(n); Buffer.from(s, "utf8").copy(b, 0, 0, n - 1); return b; };
const zeros = (n) => Buffer.alloc(n);
const seq = (start, n) => Buffer.from(Array.from({ length: n }, (_, i) => (start + i) & 0xff));
const text = (b) => {
  // Same escaping as the Rust Display (escape_debug for characters, \xNN for invalid bytes).
  let out = "";
  for (const ch of Buffer.from(b).toString("latin1")) {
    const c = ch.charCodeAt(0);
    if (c >= 0x20 && c < 0x7f && ch !== '"' && ch !== "\\") out += ch;
    else if (ch === '"') out += '\\"';
    else if (ch === "\\") out += "\\\\";
    else if (c === 0) out += "\\0";
    else if (c < 0x80) out += `\\u{${c.toString(16)}}`;
    else out += `\\x${c.toString(16).padStart(2, "0")}`;
  }
  return `"${out}"`;
};

// ---------------------------------------------------------------- COBS (Cheshire and Baker)
function cobs(data) {
  const out = [0];
  let codeAt = 0, code = 1;
  for (const b of data) {
    if (b === 0) { out[codeAt] = code; codeAt = out.length; out.push(0); code = 1; }
    else { out.push(b); code++; if (code === 0xff) { out[codeAt] = code; codeAt = out.length; out.push(0); code = 1; } }
  }
  out[codeAt] = code;
  return Buffer.from(out);
}

// ---------------------------------------------------------------- meshcore.js oracle
class Capture extends Connection {
  constructor() { super(); this.frames = []; }
  async sendToRadioFrame(data) { this.frames.push(Buffer.from(data)); }
}
let jsEncoded = 0, jsDecoded = 0;
async function jsEncode(fn) {
  const c = new Capture();
  await fn(c);
  assert.equal(c.frames.length, 1);
  jsEncoded++;
  return c.frames[0];
}
async function jsDecode(frame) {
  jsDecoded++;
  const c = new Capture();
  let got = null;
  const code = frame[0];
  c.on(code, (v) => { got = v; });
  // meshcore.js emits V3 messages under the non-V3 code.
  if (code === Constants.ResponseCodes.ContactMsgRecvV3) c.on(Constants.ResponseCodes.ContactMsgRecv, (v) => { got = v; });
  if (code === Constants.ResponseCodes.ChannelMsgRecvV3) c.on(Constants.ResponseCodes.ChannelMsgRecv, (v) => { got = v; });
  c.onFrameReceived(new Uint8Array(frame));
  // meshcore.js emits its events in a setTimeout(0): let one tick go by.
  await new Promise((r) => setTimeout(r, 1));
  assert.ok(got, `meshcore.js read nothing in ${hex(frame)}`);
  return got;
}
const same = (a, b, what) => assert.equal(hex(a), hex(b), what);

// ---------------------------------------------------------------- data
const K1 = seq(0x10, 32);
const K2 = seq(0x80, 32);
const T = 1_793_000_000;
const ENV = cat(0x10, seq(0, 8), u16(1), Buffer.from([0x00, 0x41, 0x00, 0x00]), seq(0xe0, 8)); // sample envelope, with some 0x00
const DM_TEXT = cat(0xfe, cobs(ENV));
const CH_NAME = fixed("ln-abcd", 32);
const CH_SECRET = seq(0x40, 16);
const SCOPE_KEY = createHash("sha256").update("#fr").digest().subarray(0, 16);

const commands = [], commandErrs = [], responses = [], responseErrs = [];
const okc = (name, bytes, dump) => commands.push({ name, bytes, dump });
const okr = (name, bytes, dump) => responses.push({ name, bytes, dump });

// ---------------------------------------------------------------- commands
okc("device-query", cat(0x16, 3), "DEVICE_QUERY app_ver=3");
same(cat(0x16, 3), await jsEncode((c) => c.sendCommandDeviceQuery(3)), "DEVICE_QUERY");
okc("app-start", cat(0x01, zeros(7), Buffer.from("ln")), 'APP_START reserved=00000000000000 app="ln"');
okc("set-time", cat(0x06, u32(T)), `SET_DEVICE_TIME ${T}`);
same(cat(0x06, u32(T)), await jsEncode((c) => c.sendCommandSetDeviceTime(T)), "SET_DEVICE_TIME");
okc("get-contacts", cat(0x04), "GET_CONTACTS");
same(cat(0x04), await jsEncode((c) => c.sendCommandGetContacts()), "GET_CONTACTS");
okc("get-contacts-since", cat(0x04, u32(5)), "GET_CONTACTS since=5");
same(cat(0x04, u32(5)), await jsEncode((c) => c.sendCommandGetContacts(5)), "GET_CONTACTS since");
const cname = fixed(`ln-${hex(K1.subarray(0, 4))}`, 32);
const add = cat(0x09, K1, 1, 0, 0xff, zeros(64), cname, u32(0));
okc("add-contact", add,
  `ADD_UPDATE_CONTACT key=${hex(K1)} type=1 flags=0 path_len=255 path=${hex(zeros(64))} name="ln-${hex(K1.subarray(0, 4))}" last_advert=0`);
// meshcore.js always appends lat and lon: same frame, plus 8 bytes.
same(add, (await jsEncode((c) => c.sendCommandAddUpdateContact(K1, 1, 0, 0xff, zeros(64), `ln-${hex(K1.subarray(0, 4))}`, 0, 0, 0))).subarray(0, add.length), "ADD_UPDATE_CONTACT");
okc("reset-path", cat(0x0d, K1), `RESET_PATH key=${hex(K1)}`);
same(cat(0x0d, K1), await jsEncode((c) => c.sendCommandResetPath(K1)), "RESET_PATH");
okc("advert-flood", cat(0x07, 1), "SEND_SELF_ADVERT flood=true");
same(cat(0x07, 1), await jsEncode((c) => c.sendCommandSendSelfAdvert(1)), "SEND_SELF_ADVERT");
okc("advert-zero-hop", cat(0x07, 0), "SEND_SELF_ADVERT flood=false");
same(cat(0x07, 0), await jsEncode((c) => c.sendCommandSendSelfAdvert(0)), "SEND_SELF_ADVERT zero hop");
okc("advert-no-mode", cat(0x07), "SEND_SELF_ADVERT"); // spec only (M:1305: optional byte)
const txt = cat(0x02, 0, 2, u32(T), K1.subarray(0, 6), DM_TEXT);
okc("txt-msg", txt, `SEND_TXT_MSG type=0 attempt=2 ts=${T} prefix=${hex(K1.subarray(0, 6))} text=${hex(DM_TEXT)}`);
// meshcore.js writes the text as a string: only an ASCII text is cross-checked.
same(cat(0x02, 0, 2, u32(T), K1.subarray(0, 6), Buffer.from("hi")),
  await jsEncode((c) => c.sendCommandSendTxtMsg(0, 2, T, K1, "hi")), "SEND_TXT_MSG");
okc("get-channel", cat(0x1f, 39), "GET_CHANNEL 39");
same(cat(0x1f, 39), await jsEncode((c) => c.sendCommandGetChannel(39)), "GET_CHANNEL");
okc("set-channel", cat(0x20, 39, CH_NAME, CH_SECRET), `SET_CHANNEL 39 name="ln-abcd" secret=${hex(CH_SECRET)}`);
same(cat(0x20, 39, CH_NAME, CH_SECRET), await jsEncode((c) => c.sendCommandSetChannel(39, "ln-abcd", CH_SECRET)), "SET_CHANNEL");
okc("set-channel-clear", cat(0x20, 38, zeros(32), zeros(16)), `SET_CHANNEL 38 name="" secret=${hex(zeros(16))}`);
const cdata = cat(0x3e, 39, 0xff, u16(0xff4c), ENV);
okc("channel-data", cdata, `SEND_CHANNEL_DATA 39 type=ff4c data=${hex(ENV)}`);
same(cdata, await jsEncode((c) => c.sendCommandSendChannelData(39, 0xff, new Uint8Array(0), 0xff4c, ENV)), "SEND_CHANNEL_DATA");
okc("sync", cat(0x0a), "SYNC_NEXT_MESSAGE");
same(cat(0x0a), await jsEncode((c) => c.sendCommandSyncNextMessage()), "SYNC_NEXT_MESSAGE");
const radio = cat(0x0b, u32(869618), u32(62500), 8, 8);
okc("radio", radio, "SET_RADIO_PARAMS freq=869618kHz bw=62500Hz sf=8 cr=8");
same(radio, await jsEncode((c) => c.sendCommandSetRadioParams(869618, 62500, 8, 8)), "SET_RADIO_PARAMS");
okc("get-scope", cat(0x40), "GET_DEFAULT_FLOOD_SCOPE");
okc("set-scope", cat(0x3f, fixed("fr", 31), SCOPE_KEY), `SET_DEFAULT_FLOOD_SCOPE name="fr" key=${hex(SCOPE_KEY)}`);
okc("set-scope-none", cat(0x3f), "SET_DEFAULT_FLOOD_SCOPE none");

okc("get-device-time", cat(0x05), "GET_DEVICE_TIME");
same(cat(0x05), await jsEncode((c) => c.sendCommandGetDeviceTime()), "GET_DEVICE_TIME");
okc("export-private-key", cat(0x17), "EXPORT_PRIVATE_KEY");
same(cat(0x17), await jsEncode((c) => c.sendCommandExportPrivateKey()), "EXPORT_PRIVATE_KEY");
const ikey = cat(0x18, seq(0x20, 64));
okc("import-private-key", ikey, "IMPORT_PRIVATE_KEY <64 bytes>");
same(ikey, await jsEncode((c) => c.sendCommandImportPrivateKey(seq(0x20, 64))), "IMPORT_PRIVATE_KEY");
const rawPkt = cat(0x11, 0x00, seq(0, 10));
okc("send-raw-packet", cat(0x41, 1, rawPkt), `SEND_RAW_PACKET prio=1 raw=${hex(rawPkt)}`);

const cdirect = cat(0x3e, 1, 0, u16(0xff4c), 0xaa);
okc("channel-data-zero-hop", cdirect, "SEND_CHANNEL_DATA 1 type=ff4c data=aa path=00:");
same(cdirect, await jsEncode((c) => c.sendCommandSendChannelData(1, 0, new Uint8Array(0), 0xff4c, Buffer.from([0xaa]))), "SEND_CHANNEL_DATA zero hop");
const cpath = cat(0x3e, 2, 0x42, 0x11, 0x22, 0x33, 0x44, u16(0xff00), 1, 2);
okc("channel-data-path", cpath, "SEND_CHANNEL_DATA 2 type=ff00 data=0102 path=42:11223344");
okc("remove-contact", cat(0x0f, K2), `REMOVE_CONTACT key=${hex(K2)}`);
same(cat(0x0f, K2), await jsEncode((c) => c.sendCommandRemoveContact(K2)), "REMOVE_CONTACT");
// data_type 0 and an empty payload are rejected after parsing (ERR 6 after the channel,
// M:1226-1229): the frame itself is canonical.
const cdata0 = cat(0x3e, 1, 0xff, u16(0), 0xaa);
okc("channel-data-type-0", cdata0, "SEND_CHANNEL_DATA 1 type=0000 data=aa");
same(cdata0, await jsEncode((c) => c.sendCommandSendChannelData(1, 0xff, new Uint8Array(0), 0, Buffer.from([0xaa]))), "SEND_CHANNEL_DATA type 0");
const cdataEmpty = cat(0x3e, 1, 0xff, u16(0xff4c));
okc("channel-data-empty", cdataEmpty, "SEND_CHANNEL_DATA 1 type=ff4c data=");
same(cdataEmpty, await jsEncode((c) => c.sendCommandSendChannelData(1, 0xff, new Uint8Array(0), 0xff4c, new Uint8Array(0))), "SEND_CHANNEL_DATA empty");

// All the other v1.18 commands (M:9-73).
const appStartJs = cat(0x01, 1, zeros(6), Buffer.from("test"));
okc("app-start-meshcore-js", appStartJs, 'APP_START reserved=01000000000000 app="test"');
same(appStartJs, await jsEncode((c) => c.sendCommandAppStart()), "APP_START");
okc("app-start-no-name", cat(0x01, zeros(7)), 'APP_START reserved=00000000000000 app=""'); // spec only
const chtxt = cat(0x03, 0, 1, u32(T), Buffer.from("hello"));
okc("channel-txt", chtxt, `SEND_CHANNEL_TXT_MSG 1 type=0 ts=${T} text="hello"`);
same(chtxt, await jsEncode((c) => c.sendCommandSendChannelTxtMsg(0, 1, T, "hello")), "SEND_CHANNEL_TXT_MSG");
const chtxtEmpty = cat(0x03, 0, 0, u32(T));
okc("channel-txt-empty", chtxtEmpty, `SEND_CHANNEL_TXT_MSG 0 type=0 ts=${T} text=""`);
same(chtxtEmpty, await jsEncode((c) => c.sendCommandSendChannelTxtMsg(0, 0, T, "")), "SEND_CHANNEL_TXT_MSG empty");
okc("set-advert-name", cat(0x08, Buffer.from("oxymesh")), 'SET_ADVERT_NAME "oxymesh"');
same(cat(0x08, Buffer.from("oxymesh")), await jsEncode((c) => c.sendCommandSetAdvertName("oxymesh")), "SET_ADVERT_NAME");
const addLatLon = cat(add, i32(48856614), i32(-2352222));
okc("add-contact-lat-lon", addLatLon,
  `ADD_UPDATE_CONTACT key=${hex(K1)} type=1 flags=0 path_len=255 path=${hex(zeros(64))} name="ln-${hex(K1.subarray(0, 4))}" last_advert=0 lat=48856614 lon=-2352222`);
same(addLatLon, await jsEncode((c) => c.sendCommandAddUpdateContact(K1, 1, 0, 0xff, zeros(64), `ln-${hex(K1.subarray(0, 4))}`, 0, 48856614, -2352222)), "ADD_UPDATE_CONTACT lat/lon");
const addLastmod = cat(addLatLon, u32(T));
okc("add-contact-lastmod", addLastmod, // spec only (M:208-215)
  `ADD_UPDATE_CONTACT key=${hex(K1)} type=1 flags=0 path_len=255 path=${hex(zeros(64))} name="ln-${hex(K1.subarray(0, 4))}" last_advert=0 lat=48856614 lon=-2352222 lastmod=${T}`);
okc("radio-repeat", cat(radio, 1), "SET_RADIO_PARAMS freq=869618kHz bw=62500Hz sf=8 cr=8 repeat=1"); // spec only (M:1434)
okc("tx-power", cat(0x0c, 22), "SET_RADIO_TX_POWER 22dBm");
same(cat(0x0c, 22), await jsEncode((c) => c.sendCommandSetTxPower(22)), "SET_RADIO_TX_POWER");
okc("tx-power-negative", cat(0x0c, 0xf7), "SET_RADIO_TX_POWER -9dBm");
same(cat(0x0c, 0xf7), await jsEncode((c) => c.sendCommandSetTxPower(-9)), "SET_RADIO_TX_POWER -9");
const latlon = cat(0x0e, i32(48856614), i32(-2352222));
okc("advert-latlon", latlon, "SET_ADVERT_LATLON lat=48856614 lon=-2352222");
same(latlon, await jsEncode((c) => c.sendCommandSetAdvertLatLon(48856614, -2352222)), "SET_ADVERT_LATLON");
okc("advert-latlon-alt", cat(latlon, i32(-12)), "SET_ADVERT_LATLON lat=48856614 lon=-2352222 alt=-12"); // spec only
okc("share-contact", cat(0x10, K2), `SHARE_CONTACT key=${hex(K2)}`);
same(cat(0x10, K2), await jsEncode((c) => c.sendCommandShareContact(K2)), "SHARE_CONTACT");
okc("export-contact-self", cat(0x11), "EXPORT_CONTACT self");
same(cat(0x11), await jsEncode((c) => c.sendCommandExportContact()), "EXPORT_CONTACT self");
okc("export-contact", cat(0x11, K2), `EXPORT_CONTACT key=${hex(K2)}`);
same(cat(0x11, K2), await jsEncode((c) => c.sendCommandExportContact(K2)), "EXPORT_CONTACT");
// Raw advert: FLOOD|ADVERT header, path_len 0, key, ts, signature, advert data.
const ADVERT_RAW = cat(0x11, 0x00, K1, u32(T), seq(0x40, 64), 0x81, Buffer.from("x"));
assert.equal(ADVERT_RAW.length, 104);
okc("import-contact", cat(0x12, ADVERT_RAW), `IMPORT_CONTACT raw=${hex(ADVERT_RAW)}`);
same(cat(0x12, ADVERT_RAW), await jsEncode((c) => c.sendCommandImportContact(ADVERT_RAW)), "IMPORT_CONTACT");
okc("reboot", cat(0x13, Buffer.from("reboot")), "REBOOT");
same(cat(0x13, Buffer.from("reboot")), await jsEncode((c) => c.sendCommandReboot()), "REBOOT");
okc("get-batt-and-storage", cat(0x14), "GET_BATT_AND_STORAGE");
same(cat(0x14), await jsEncode((c) => c.sendCommandGetBatteryVoltage()), "GET_BATT_AND_STORAGE");
okc("set-tuning", cat(0x15, u32(0), u32(1000)), "SET_TUNING_PARAMS rx_delay=0/1000 airtime_factor=1000/1000"); // spec only
const rawData = cat(0x19, 0x02, 0xaa, 0xbb, seq(1, 6));
okc("send-raw-data", rawData, "SEND_RAW_DATA path=02:aabb payload=010203040506");
same(rawData, await jsEncode((c) => c.sendCommandSendRawData(2, Buffer.from([0xaa, 0xbb]), seq(1, 6))), "SEND_RAW_DATA");
okc("send-raw-data-zero-hop", cat(0x19, 0, 1, 2, 3, 4), "SEND_RAW_DATA path=00: payload=01020304");
same(cat(0x19, 0, 1, 2, 3, 4), await jsEncode((c) => c.sendCommandSendRawData(0, new Uint8Array(0), Buffer.from([1, 2, 3, 4]))), "SEND_RAW_DATA zero hop");
okc("send-raw-data-hash-2", cat(0x19, 0x41, 0xaa, 0xbb, 1, 2, 3, 4), "SEND_RAW_DATA path=41:aabb payload=01020304"); // spec only
okc("send-login", cat(0x1a, K2, Buffer.from("pass")), `SEND_LOGIN key=${hex(K2)} password=<4 bytes>`);
same(cat(0x1a, K2, Buffer.from("pass")), await jsEncode((c) => c.sendCommandSendLogin(K2, "pass")), "SEND_LOGIN");
okc("send-login-no-password", cat(0x1a, K2), `SEND_LOGIN key=${hex(K2)} password=<0 bytes>`);
same(cat(0x1a, K2), await jsEncode((c) => c.sendCommandSendLogin(K2, "")), "SEND_LOGIN empty");
okc("send-status-req", cat(0x1b, K2), `SEND_STATUS_REQ key=${hex(K2)}`);
same(cat(0x1b, K2), await jsEncode((c) => c.sendCommandSendStatusReq(K2)), "SEND_STATUS_REQ");
okc("has-connection", cat(0x1c, K2), `HAS_CONNECTION key=${hex(K2)}`); // spec only
okc("logout", cat(0x1d, K2), `LOGOUT key=${hex(K2)}`); // spec only
okc("get-contact-by-key", cat(0x1e, K2), `GET_CONTACT_BY_KEY key=${hex(K2)}`); // spec only
okc("sign-start", cat(0x21), "SIGN_START");
same(cat(0x21), await jsEncode((c) => c.sendCommandSignStart()), "SIGN_START");
okc("sign-data", cat(0x22, seq(0, 5)), "SIGN_DATA 0001020304");
same(cat(0x22, seq(0, 5)), await jsEncode((c) => c.sendCommandSignData(seq(0, 5))), "SIGN_DATA");
okc("sign-finish", cat(0x23), "SIGN_FINISH");
same(cat(0x23), await jsEncode((c) => c.sendCommandSignFinish()), "SIGN_FINISH");
const trace = cat(0x24, u32(7), u32(9), 0, 0x11, 0x22, 0x33);
okc("trace", trace, "SEND_TRACE_PATH tag=7 auth=9 flags=0 path=112233");
same(trace, await jsEncode((c) => c.sendCommandSendTracePath(7, 9, Buffer.from([0x11, 0x22, 0x33]))), "SEND_TRACE_PATH");
okc("trace-hash-2", cat(0x24, u32(7), u32(9), 1, 0x11, 0x22, 0x33, 0x44), "SEND_TRACE_PATH tag=7 auth=9 flags=1 path=11223344"); // spec only
okc("trace-hash-8", cat(0x24, u32(7), u32(9), 3, seq(0, 8)), "SEND_TRACE_PATH tag=7 auth=9 flags=3 path=0001020304050607"); // spec only (flags & 3 = 3: 8 bytes, M:1816)
okc("set-device-pin", cat(0x25, u32(123456)), "SET_DEVICE_PIN 123456"); // spec only
okc("set-other-params", cat(0x26, 1), "SET_OTHER_PARAMS manual_add=1");
same(cat(0x26, 1), await jsEncode((c) => c.sendCommandSetOtherParams(1)), "SET_OTHER_PARAMS");
okc("set-other-params-full", cat(0x26, 1, 0x15, 1, 2), "SET_OTHER_PARAMS manual_add=1 telemetry=0x15 advert_loc_policy=1 multi_acks=2"); // spec only
okc("set-other-params-telemetry", cat(0x26, 0, 0x02), "SET_OTHER_PARAMS manual_add=0 telemetry=0x02"); // spec only
okc("telemetry-req", cat(0x27, 0, 0, 0, K2), `SEND_TELEMETRY_REQ reserved=000000 key=${hex(K2)}`);
same(cat(0x27, 0, 0, 0, K2), await jsEncode((c) => c.sendCommandSendTelemetryReq(K2)), "SEND_TELEMETRY_REQ");
okc("telemetry-req-self", cat(0x27, 0, 0, 0), "SEND_TELEMETRY_REQ reserved=000000 self"); // spec only (M:1700)
okc("get-custom-vars", cat(0x28), "GET_CUSTOM_VARS"); // spec only
okc("set-custom-var", cat(0x29, Buffer.from("gps:1")), 'SET_CUSTOM_VAR "gps:1"'); // spec only
okc("get-advert-path", cat(0x2a, 0, K2), `GET_ADVERT_PATH reserved=0 key=${hex(K2)}`); // spec only
okc("get-tuning", cat(0x2b), "GET_TUNING_PARAMS"); // spec only
const binReq = cat(0x32, K2, 0x03, 0, 0, 0, 0);
okc("binary-req", binReq, `SEND_BINARY_REQ key=${hex(K2)} data=0300000000`);
same(binReq, await jsEncode((c) => c.sendCommandSendBinaryReq(K2, Buffer.from([3, 0, 0, 0, 0]))), "SEND_BINARY_REQ");
okc("factory-reset", cat(0x33, Buffer.from("reset")), "FACTORY_RESET"); // spec only
okc("path-discovery", cat(0x34, 0, K2), `SEND_PATH_DISCOVERY_REQ key=${hex(K2)}`); // spec only
okc("flood-scope-key", cat(0x36, 0, SCOPE_KEY), `SET_FLOOD_SCOPE_KEY key=${hex(SCOPE_KEY)}`);
same(cat(0x36, 0, SCOPE_KEY), await jsEncode((c) => c.sendCommandSetFloodScope(SCOPE_KEY)), "SET_FLOOD_SCOPE_KEY");
okc("flood-scope-clear", cat(0x36, 0), "SET_FLOOD_SCOPE_KEY clear");
same(cat(0x36, 0), await jsEncode((c) => c.sendCommandSetFloodScope(new Uint8Array(0))), "SET_FLOOD_SCOPE_KEY cleared");
okc("flood-scope-unscoped", cat(0x36, 1), "SET_FLOOD_SCOPE_KEY unscoped"); // spec only (v12+, M:1986)
okc("control-data", cat(0x37, 0x81, 1, 2), "SEND_CONTROL_DATA 810102"); // spec only
okc("get-stats", cat(0x38, 2), "GET_STATS 2");
same(cat(0x38, 2), await jsEncode((c) => c.sendCommandGetStats(2)), "GET_STATS");
okc("anon-req", cat(0x39, K2, 1, 2, 3), `SEND_ANON_REQ key=${hex(K2)} data=010203`); // spec only
okc("set-autoadd", cat(0x3a, 0x1e), "SET_AUTOADD_CONFIG config=0x1e"); // spec only
okc("set-autoadd-hops", cat(0x3a, 0x1f, 3), "SET_AUTOADD_CONFIG config=0x1f max_hops=3"); // spec only
okc("get-autoadd", cat(0x3b), "GET_AUTOADD_CONFIG"); // spec only
okc("get-repeat-freq", cat(0x3c), "GET_ALLOWED_REPEAT_FREQ"); // spec only
okc("set-path-hash-mode", cat(0x3d, 0, 1), "SET_PATH_HASH_MODE 1"); // spec only
okc("run-cli", cat(0x42, Buffer.from("ver")), 'RUN_CLI_COMMAND "ver"'); // spec only

const errc = (name, bytes, kind) => commandErrs.push({ name, bytes, kind });
errc("send-raw-packet-short", cat(0x41, 1, 0x11), "Truncated");
errc("import-private-key-short", cat(0x18, seq(0, 63)), "Truncated");
errc("get-device-time-trailing", cat(0x05, 0), "Length");
errc("empty", Buffer.alloc(0), "Truncated");
errc("unknown", cat(0x99), "Code");
errc("txt-no-text", cat(0x02, 0, 0, u32(T), K1.subarray(0, 6)), "Truncated");
errc("advert-mode-2", cat(0x07, 2), "Value");
errc("advert-trailing", cat(0x07, 1, 0), "Length");
errc("channel-data-hash-size-4", cat(0x3e, 1, 0xc1, 0, 0, 0, 0, u16(0xff4c), 0xaa), "Value");
errc("channel-data-path-65", cat(0x3e, 1, 0x61, zeros(66), u16(0xff4c), 0xaa), "Value");
errc("channel-data-path-short", cat(0x3e, 1, 0x02, 0x11), "Truncated");
errc("channel-data-no-type", cat(0x3e, 1, 0xff, 0x4c), "Truncated");
errc("device-query-trailing", cat(0x16, 3, 0), "Length");
errc("add-contact-short", cat(0x09, K1.subarray(0, 10)), "Truncated");
errc("add-contact-lat-only", cat(add, i32(0)), "Truncated");
errc("add-contact-lastmod-short", cat(addLatLon, 0, 0), "Truncated");
errc("add-contact-trailing", cat(addLastmod, 0), "Length");
errc("set-scope-short", cat(0x3f, fixed("fr", 31)), "Truncated");
errc("too-long", cat(0x02, 0, 0, u32(T), K1.subarray(0, 6), Buffer.alloc(164, 0x41)), "Length");
errc("code-0", cat(0x00), "Code");
errc("reserved-44", cat(0x2c), "Code");
errc("reserved-49", cat(0x31), "Code");
errc("reserved-53", cat(0x35, 0), "Code");
errc("code-67", cat(0x43), "Code");
errc("channel-txt-short", cat(0x03, 0, 1, 0, 0), "Truncated");
errc("set-advert-name-empty", cat(0x08), "Truncated");
errc("radio-trailing", cat(radio, 1, 0), "Length");
errc("tx-power-empty", cat(0x0c), "Truncated");
errc("advert-latlon-alt-short", cat(latlon, 0, 0), "Truncated");
errc("export-contact-short", cat(0x11, K2.subarray(0, 10)), "Truncated");
errc("import-contact-short", cat(0x12, ADVERT_RAW.subarray(0, 97)), "Truncated");
errc("reboot-wrong", cat(0x13, Buffer.from("rebooT")), "Value");
errc("reboot-short", cat(0x13, Buffer.from("reb")), "Truncated");
errc("reboot-trailing", cat(0x13, Buffer.from("reboot"), 0), "Length");
errc("raw-data-flood", cat(0x19, 0xff, 1, 2, 3, 4), "Value");
errc("raw-data-short-payload", cat(0x19, 0, 1, 2, 3), "Truncated");
errc("raw-data-path-short", cat(0x19, 3, 0xaa), "Truncated");
errc("login-short", cat(0x1a, K2.subarray(0, 31)), "Truncated");
errc("set-channel-32-key", cat(0x20, 1, CH_NAME, seq(0, 32)), "Length");
errc("sign-data-empty", cat(0x22), "Truncated");
errc("trace-no-path", cat(0x24, u32(7), u32(9), 0), "Truncated");
errc("trace-odd", cat(0x24, u32(7), u32(9), 1, 1, 2, 3), "Value");
errc("trace-65-hops", cat(0x24, u32(7), u32(9), 0, seq(0, 65)), "Value");
errc("set-other-params-trailing", cat(0x26, 1, 2, 3, 4, 5), "Length");
errc("telemetry-req-partial-key", cat(0x27, 0, 0, 0, K2.subarray(0, 10)), "Truncated");
errc("set-custom-var-short", cat(0x29, Buffer.from("a:")), "Truncated");
errc("factory-reset-wrong", cat(0x33, Buffer.from("RESET")), "Value");
errc("path-discovery-reserved", cat(0x34, 1, K2), "Value");
errc("flood-scope-2", cat(0x36, 2), "Value");
errc("flood-scope-short-key", cat(0x36, 0, seq(1, 10)), "Truncated");
errc("flood-scope-unscoped-trailing", cat(0x36, 1, 0), "Length");
errc("flood-scope-empty", cat(0x36), "Truncated");
errc("control-data-no-flag", cat(0x37, 0x10, 1), "Value");
errc("control-data-empty", cat(0x37), "Truncated");
errc("autoadd-trailing", cat(0x3a, 1, 2, 3), "Length");
errc("path-hash-mode-reserved", cat(0x3d, 1, 1), "Value");
errc("anon-req-no-data", cat(0x39, K2), "Truncated");
errc("binary-req-no-data", cat(0x32, K2), "Truncated");
errc("run-cli-short", cat(0x42, Buffer.from("v")), "Truncated");

// ---------------------------------------------------------------- responses
const contactBody = (name, extra = Buffer.alloc(0)) =>
  cat(K2, 2, 1, 2, cat(Buffer.from([0xaa, 0xbb]), zeros(62)), fixed(name, 32), u32(T - 60), i32(48856614), i32(2352222), u32(T - 30), extra);
const contactDump = (name, ext = "") =>
  `key=${hex(K2)} type=2 flags=1 path_len=2 name="${name}" last_advert=${T - 60} lat=48856614 lon=2352222 lastmod=${T - 30}${ext}`;

okr("ok", cat(0x00), "OK");
okr("err", cat(0x01, 2), "ERR 2");
assert.equal((await jsDecode(cat(0x01, 2))).errCode, 2);
okr("contacts-start", cat(0x02, u32(3)), "CONTACTS_START total=3");
assert.equal((await jsDecode(cat(0x02, u32(3)))).count, 3);
const contact = cat(0x03, contactBody("Relay-1"));
okr("contact", contact, `CONTACT ${contactDump("Relay-1")}`);
{
  const v = await jsDecode(contact);
  same(v.publicKey, K2, "CONTACT key");
  assert.deepEqual([v.type, v.flags, v.outPathLen, v.advName, v.lastAdvert, v.advLat, v.advLon, v.lastMod],
    [2, 1, 2, "Relay-1", T - 60, 48856614, 2352222, T - 30]);
}
okr("contact-ext", cat(0x03, contactBody("Relay-1", Buffer.from([1, 2]))), `CONTACT ${contactDump("Relay-1", " ext=0102")}`);
okr("end-of-contacts", cat(0x04, u32(T - 30)), `END_OF_CONTACTS lastmod=${T - 30}`);
assert.equal((await jsDecode(cat(0x04, u32(T - 30)))).mostRecentLastmod, T - 30);
const selfInfo = cat(0x05, 1, 22, 22, K1, i32(0), i32(0), 0, 0, 0, 0, u32(869618), u32(62500), 8, 8, Buffer.from("MeshCore-cddf"));
okr("self-info", selfInfo,
  `SELF_INFO type=1 tx=22/22 key=${hex(K1)} lat=0 lon=0 manual_add=0 freq=869618kHz bw=62500Hz sf=8 cr=8 name="MeshCore-cddf"`);
{
  const v = await jsDecode(selfInfo);
  same(v.publicKey, K1, "SELF_INFO key");
  assert.deepEqual([v.type, v.txPower, v.maxTxPower, v.manualAddContacts, v.radioFreq, v.radioBw, v.radioSf, v.radioCr, v.name],
    [1, 22, 22, 0, 869618, 62500, 8, 8, "MeshCore-cddf"]);
}
const sent = cat(0x06, 0, Buffer.from([0xde, 0xad, 0xbe, 0xef]), u32(5000));
okr("sent", sent, "SENT flood=false ack=deadbeef timeout=5000ms");
{
  const v = await jsDecode(sent);
  assert.deepEqual([v.result, v.expectedAckCrc, v.estTimeout], [0, 0xefbeadde, 5000]);
}
okr("sent-flood", cat(0x06, 1, Buffer.from([1, 2, 3, 4]), u32(12000)), "SENT flood=true ack=01020304 timeout=12000ms");
okr("no-more", cat(0x0a), "NO_MORE_MESSAGES");
const devInfo = cat(0x0d, 14, 175, 40, u32(123456), fixed("15 Oct 2026", 12), fixed("Elecrow ThinkNode M5", 40), fixed("v1.18.0", 20), 0, 0);
okr("device-info", devInfo,
  'DEVICE_INFO fw=14 contacts=350 channels=40 pin=123456 build="15 Oct 2026" maker="Elecrow ThinkNode M5" version="v1.18.0" repeat=0 path_hash_mode=0');
{
  const v = await jsDecode(devInfo);
  assert.deepEqual([v.firmwareVer, v.firmware_build_date], [14, "15 Oct 2026"]);
}
okr("device-info-ext", cat(devInfo, 7),
  'DEVICE_INFO fw=14 contacts=350 channels=40 pin=123456 build="15 Oct 2026" maker="Elecrow ThinkNode M5" version="v1.18.0" repeat=0 path_hash_mode=0 ext=07');
const cmsg = cat(0x10, 0xec, 0, 0, K2.subarray(0, 6), 3, 0, u32(T), DM_TEXT);
okr("contact-msg", cmsg, `CONTACT_MSG snr4=-20 prefix=${hex(K2.subarray(0, 6))} path_len=3 type=0 ts=${T} text=${hex(DM_TEXT)}`);
const cmsgPlain = cat(0x10, 0x28, 0, 0, K2.subarray(0, 6), 0xff, 0, u32(T), Buffer.from("hello"));
okr("contact-msg-plain", cmsgPlain, `CONTACT_MSG snr4=40 prefix=${hex(K2.subarray(0, 6))} path_len=255 type=0 ts=${T} text=${hex(Buffer.from("hello"))}`);
{
  const v = await jsDecode(cmsgPlain);
  same(v.pubKeyPrefix, K2.subarray(0, 6), "CONTACT_MSG prefix");
  assert.deepEqual([v.snr, v.pathLen, v.txtType, v.senderTimestamp, v.text], [10, 0xff, 0, T, "hello"]);
}
const chmsg = cat(0x11, 0x14, 0, 0, 0, 1, 0, u32(T), Buffer.from("alice: hi"));
okr("channel-msg", chmsg, `CHANNEL_MSG snr4=20 idx=0 path_len=1 type=0 ts=${T} text="alice: hi"`);
{
  const v = await jsDecode(chmsg);
  assert.deepEqual([v.channelIdx, v.pathLen, v.txtType, v.senderTimestamp, v.text], [0, 1, 0, T, "alice: hi"]);
}
const chinfo = cat(0x12, 39, CH_NAME, CH_SECRET);
okr("channel-info", chinfo, `CHANNEL_INFO 39 name="ln-abcd" secret=${hex(CH_SECRET)}`);
{
  const v = await jsDecode(chinfo);
  assert.deepEqual([v.channelIdx, v.name, hex(v.secret)], [39, "ln-abcd", hex(CH_SECRET)]);
}
okr("channel-info-empty", cat(0x12, 5, zeros(32), zeros(16)), `CHANNEL_INFO 5 name="" secret=${hex(zeros(16))}`);
const chdata = cat(0x1b, 0xf8, 0, 0, 39, 2, u16(0xff4c), ENV.length, ENV);
okr("channel-data", chdata, `CHANNEL_DATA snr4=-8 idx=39 path_len=2 type=ff4c data=${hex(ENV)}`);
{
  const v = await jsDecode(chdata);
  assert.deepEqual([v.snr, v.channelIdx, v.pathLen, v.dataType, v.dataLen, hex(v.data)], [-2, 39, 2, 0xff4c, ENV.length, hex(ENV)]);
}
okr("scope", cat(0x1c, fixed("fr", 31), SCOPE_KEY), `DEFAULT_FLOOD_SCOPE name="fr" key=${hex(SCOPE_KEY)}`);
okr("scope-none", cat(0x1c), "DEFAULT_FLOOD_SCOPE none");
okr("advert", cat(0x80, K2), `ADVERT key=${hex(K2)}`);
same((await jsDecode(cat(0x80, K2))).publicKey, K2, "ADVERT");
okr("path-updated", cat(0x81, K2), `PATH_UPDATED key=${hex(K2)}`);
same((await jsDecode(cat(0x81, K2))).publicKey, K2, "PATH_UPDATED");
const conf = cat(0x82, Buffer.from([0xde, 0xad, 0xbe, 0xef]), u32(1234));
okr("send-confirmed", conf, "SEND_CONFIRMED ack=deadbeef rtt=1234ms");
{
  const v = await jsDecode(conf);
  assert.deepEqual([v.ackCode, v.roundTrip], [0xefbeadde, 1234]);
}
okr("msg-waiting", cat(0x83), "MSG_WAITING");
okr("new-advert", cat(0x8a, contactBody("Bob")), `NEW_ADVERT ${contactDump("Bob")}`);
okr("contact-deleted", cat(0x8f, K2), `CONTACT_DELETED key=${hex(K2)}`);
okr("contacts-full", cat(0x90), "CONTACTS_FULL");
const logrx = cat(0x88, 0x2c, 0xb0, 0x11, 0x22);
okr("log-rx-data", logrx, "LOG_RX_DATA snr4=44 rssi=-80 raw=1122");
{
  const v = await jsDecode(logrx);
  assert.deepEqual([v.lastSnr, v.lastRssi, hex(v.raw)], [11, -80, "1122"]);
}
const ctime = cat(0x09, u32(T));
okr("curr-time", ctime, `CURR_TIME ${T}`);
assert.equal((await jsDecode(ctime)).epochSecs, T);
const pkey = cat(0x0e, seq(0x20, 64));
okr("private-key", pkey, "PRIVATE_KEY <64 bytes>");
assert.equal(hex((await jsDecode(pkey)).privateKey), hex(seq(0x20, 64)));
okr("disabled", cat(0x0f), "DISABLED");
await jsDecode(cat(0x0f));
okr("other-unknown", cat(0x99, 1, 2), "OTHER code=0x99 body=0102");
okr("other-30", cat(0x1e), "OTHER code=0x1e body=");
okr("other-0x91", cat(0x91, 0), "OTHER code=0x91 body=00");

// All the other v1.18 responses and notifications (M:76-134).
const cmsgLegacy = cat(0x07, K2.subarray(0, 6), 0xff, 0, u32(T), Buffer.from("hello"));
okr("contact-msg-legacy", cmsgLegacy, `CONTACT_MSG_LEGACY prefix=${hex(K2.subarray(0, 6))} path_len=255 type=0 ts=${T} text=${hex(Buffer.from("hello"))}`);
{
  const v = await jsDecode(cmsgLegacy);
  same(v.pubKeyPrefix, K2.subarray(0, 6), "CONTACT_MSG legacy prefix");
  assert.deepEqual([v.snr, v.pathLen, v.txtType, v.senderTimestamp, v.text], [null, 0xff, 0, T, "hello"]);
}
const chmsgLegacy = cat(0x08, 2, 3, 0, u32(T), Buffer.from("bob: yo"));
okr("channel-msg-legacy", chmsgLegacy, `CHANNEL_MSG_LEGACY idx=2 path_len=3 type=0 ts=${T} text="bob: yo"`);
{
  const v = await jsDecode(chmsgLegacy);
  assert.deepEqual([v.snr, v.channelIdx, v.pathLen, v.txtType, v.senderTimestamp, v.text], [null, 2, 3, 0, T, "bob: yo"]);
}
okr("export-contact", cat(0x0b, ADVERT_RAW), `EXPORT_CONTACT raw=${hex(ADVERT_RAW)}`);
assert.equal(hex((await jsDecode(cat(0x0b, ADVERT_RAW))).advertPacketBytes), hex(ADVERT_RAW));
const batt = cat(0x0c, u16(4123), u32(12), u32(1408));
okr("batt-and-storage", batt, "BATT_AND_STORAGE battery=4123mV used=12kB total=1408kB");
// meshcore.js 1.15.0 only reads the voltage (old GET_BATTERY_VOLTAGE).
assert.equal((await jsDecode(batt)).batteryMilliVolts, 4123);
const signStart = cat(0x13, 0, u32(8192));
okr("sign-start", signStart, "SIGN_START max_len=8192");
{
  const v = await jsDecode(signStart);
  assert.deepEqual([v.reserved, v.maxSignDataLen], [0, 8192]);
}
const SIG = seq(0x90, 64);
okr("signature", cat(0x14, SIG), `SIGNATURE ${hex(SIG)}`);
same((await jsDecode(cat(0x14, SIG))).signature, SIG, "SIGNATURE");
okr("custom-vars", cat(0x15, Buffer.from("gps:0")), 'CUSTOM_VARS "gps:0"'); // spec only
okr("custom-vars-empty", cat(0x15), 'CUSTOM_VARS ""'); // spec only
okr("advert-path", cat(0x16, u32(T), 0x42, 0x11, 0x22, 0x33, 0x44), `ADVERT_PATH recv_ts=${T} path=42:11223344`); // spec only
okr("advert-path-direct", cat(0x16, u32(T), 0), `ADVERT_PATH recv_ts=${T} path=00:`); // spec only
okr("tuning-params", cat(0x17, u32(0), u32(1000)), "TUNING_PARAMS rx_delay=0/1000 airtime_factor=1000/1000"); // spec only
const statsCore = cat(0x18, 0, u16(4123), u32(3600), u16(0x0102), 3);
okr("stats-core", statsCore, "STATS core battery=4123mV uptime=3600s err_flags=0x0102 queue=3");
{
  // meshcore.js 1.15.0 does not know err_flags (added later): only the start is cross-checked.
  const v = await jsDecode(statsCore);
  assert.deepEqual([v.type, v.data.batteryMilliVolts, v.data.uptimeSecs, hex(v.raw)], [0, 4123, 3600, hex(statsCore.subarray(2))]);
}
const statsRadio = cat(0x18, 1, Buffer.from([0x8c, 0xff]), 0xb0, 0x1c, u32(42), u32(4242));
okr("stats-radio", statsRadio, "STATS radio noise_floor=-116 rssi=-80 snr4=28 tx_air=42s rx_air=4242s");
{
  const v = await jsDecode(statsRadio);
  assert.deepEqual([v.type, v.data.noiseFloor, v.data.lastRssi, v.data.lastSnr, v.data.txAirSecs, v.data.rxAirSecs], [1, -116, -80, 7, 42, 4242]);
}
const statsPkts = cat(0x18, 2, u32(1), u32(2), u32(3), u32(4), u32(5), u32(6), u32(7));
okr("stats-packets", statsPkts, "STATS packets recv=1 sent=2 sent_flood=3 sent_direct=4 recv_flood=5 recv_direct=6 recv_errors=7");
{
  const d = (await jsDecode(statsPkts)).data;
  assert.deepEqual([d.recv, d.sent, d.nSentFlood, d.nSentDirect, d.nRecvFlood, d.nRecvDirect, d.nRecvErrors], [1, 2, 3, 4, 5, 6, 7]);
}
okr("autoadd-config", cat(0x19, 0x1f, 3), "AUTOADD_CONFIG config=0x1f max_hops=3"); // spec only
const freqs = cat(u32(433000), u32(433000), u32(869495), u32(869495), u32(918000), u32(918000));
okr("allowed-repeat-freq", cat(0x1a, freqs), "ALLOWED_REPEAT_FREQ 433000-433000kHz 869495-869495kHz 918000-918000kHz"); // spec only (M:1032-1040)
okr("allowed-repeat-freq-none", cat(0x1a), "ALLOWED_REPEAT_FREQ"); // spec only
okr("cli-reply", cat(0x1d, Buffer.from("> v1.18.0")), 'CLI_REPLY "> v1.18.0"'); // spec only
const rawDataPush = cat(0x84, 0x28, 0xb5, 0xff, seq(1, 5));
okr("raw-data", rawDataPush, "RAW_DATA snr4=40 rssi=-75 reserved=0xff payload=0102030405");
{
  const v = await jsDecode(rawDataPush);
  assert.deepEqual([v.lastSnr, v.lastRssi, v.reserved, hex(v.payload)], [10, -75, 0xff, "0102030405"]);
}
const loginNew = cat(0x85, 1, K2.subarray(0, 6), u32(T), 3, 2);
okr("login-success", loginNew, `LOGIN_SUCCESS permissions=1 prefix=${hex(K2.subarray(0, 6))} server_time=${T} acl=3 fw_level=2`);
{
  // meshcore.js 1.15.0 only reads the old format (8 bytes).
  const v = await jsDecode(loginNew);
  assert.deepEqual([v.reserved, hex(v.pubKeyPrefix)], [1, hex(K2.subarray(0, 6))]);
}
const loginLegacy = cat(0x85, 0, K2.subarray(0, 6));
okr("login-success-legacy", loginLegacy, `LOGIN_SUCCESS permissions=0 prefix=${hex(K2.subarray(0, 6))} legacy`);
{
  const v = await jsDecode(loginLegacy);
  assert.deepEqual([v.reserved, hex(v.pubKeyPrefix)], [0, hex(K2.subarray(0, 6))]);
}
okr("login-fail", cat(0x86, 0, K2.subarray(0, 6)), `LOGIN_FAIL prefix=${hex(K2.subarray(0, 6))}`); // spec only
const status = cat(0x87, 0, K2.subarray(0, 6), seq(0x30, 10));
okr("status-response", status, `STATUS_RESPONSE prefix=${hex(K2.subarray(0, 6))} data=${hex(seq(0x30, 10))}`);
{
  const v = await jsDecode(status);
  assert.deepEqual([v.reserved, hex(v.pubKeyPrefix), hex(v.statusData)], [0, hex(K2.subarray(0, 6)), hex(seq(0x30, 10))]);
}
const traceData = cat(0x89, 0, 3, 0, u32(7), u32(9), 0x11, 0x22, 0x33, 0x28, 0x14, 0xf8, 0x0c);
okr("trace-data", traceData, "TRACE_DATA flags=0 tag=7 auth=9 hashes=112233 snrs=2814f8 final_snr4=12");
{
  const v = await jsDecode(traceData);
  assert.deepEqual([v.reserved, v.pathLen, v.flags, v.tag, v.authCode, hex(v.pathHashes), hex(v.pathSnrs), v.lastSnr],
    [0, 3, 0, 7, 9, "112233", "2814f8", 3]);
}
// flags & 3 = 1: 2-byte hashes, one SNR per hop (M:858-875). meshcore.js 1.15.0 reads as many
// SNRs as hash bytes: spec only.
okr("trace-data-hash-2", cat(0x89, 0, 4, 1, u32(7), u32(9), 0x11, 0x22, 0x33, 0x44, 0x28, 0x14, 0x0c),
  "TRACE_DATA flags=1 tag=7 auth=9 hashes=11223344 snrs=2814 final_snr4=12");
const telem = cat(0x8b, 0, K2.subarray(0, 6), Buffer.from([1, 0x74, 0x01, 0x9f]));
okr("telemetry-response", telem, `TELEMETRY_RESPONSE prefix=${hex(K2.subarray(0, 6))} lpp=0174019f`);
{
  const v = await jsDecode(telem);
  assert.deepEqual([v.reserved, hex(v.pubKeyPrefix), hex(v.lppSensorData)], [0, hex(K2.subarray(0, 6)), "0174019f"]);
}
const binResp = cat(0x8c, 0, Buffer.from([0xde, 0xad, 0xbe, 0xef]), 1, 2, 3);
okr("binary-response", binResp, "BINARY_RESPONSE tag=deadbeef data=010203");
{
  const v = await jsDecode(binResp);
  assert.deepEqual([v.reserved, v.tag, hex(v.responseData)], [0, 0xefbeadde, "010203"]);
}
okr("path-discovery-response", cat(0x8d, 0, K2.subarray(0, 6), 2, 0x11, 0x22, 0x41, 0xaa, 0xbb),
  `PATH_DISCOVERY_RESPONSE prefix=${hex(K2.subarray(0, 6))} out=02:1122 in=41:aabb`); // spec only
okr("path-discovery-response-direct", cat(0x8d, 0, K2.subarray(0, 6), 0, 0),
  `PATH_DISCOVERY_RESPONSE prefix=${hex(K2.subarray(0, 6))} out=00: in=00:`); // spec only
okr("control-data", cat(0x8e, 0x28, 0xb5, 0, 0x81, 1, 2), "CONTROL_DATA snr4=40 rssi=-75 path_len=0 payload=810102"); // spec only

const errr = (name, bytes, kind) => responseErrs.push({ name, bytes, kind });
errr("empty", Buffer.alloc(0), "Truncated");
errr("ok-trailing", cat(0x00, 0), "Length");
errr("err-no-code", cat(0x01), "Truncated");
errr("sent-flood-2", cat(0x06, 2, zeros(4), u32(1)), "Value");
errr("sent-trailing", cat(sent, 0), "Length");
errr("contact-short", cat(0x03, K2), "Truncated");
errr("channel-info-short", cat(0x12, 39, CH_NAME), "Truncated");
errr("channel-info-trailing", cat(chinfo, 0), "Length");
errr("channel-data-short", cat(0x1b, 0, 0, 0, 39, 2, u16(0xff4c), 5, 1, 2, 3), "Truncated");
errr("channel-data-trailing", cat(0x1b, 0, 0, 0, 39, 2, u16(0xff4c), 2, 1, 2, 3), "Length");
errr("scope-short", cat(0x1c, fixed("fr", 31)), "Truncated");
errr("private-key-short", cat(0x0e, seq(0, 63)), "Truncated");
errr("curr-time-trailing", cat(ctime, 0), "Length");
errr("log-rx-data-short", cat(0x88, 0x2c), "Truncated");
errr("too-long", cat(0x10, 0, 0, 0, K2.subarray(0, 6), 0, 0, u32(T), Buffer.alloc(161, 0x41)), "Length");
errr("contact-msg-legacy-short", cat(0x07, K2.subarray(0, 6), 0xff, 0, 1, 2), "Truncated");
errr("channel-msg-legacy-short", cat(0x08, 2, 3), "Truncated");
errr("batt-short", cat(0x0c, u16(4123), u32(12)), "Truncated");
errr("batt-trailing", cat(batt, 0), "Length");
errr("sign-start-trailing", cat(signStart, 0), "Length");
errr("signature-short", cat(0x14, SIG.subarray(0, 63)), "Truncated");
errr("advert-path-bad-len", cat(0x16, u32(T), 0xff), "Value");
errr("advert-path-short", cat(0x16, u32(T), 0x02, 0x11), "Truncated");
errr("advert-path-trailing", cat(0x16, u32(T), 0x01, 0x11, 0x22), "Length");
errr("tuning-params-trailing", cat(0x17, u32(0), u32(1000), 0), "Length");
errr("stats-type-3", cat(0x18, 3, u32(0)), "Value");
errr("stats-core-short", statsCore.subarray(0, statsCore.length - 1), "Truncated");
errr("stats-packets-trailing", cat(statsPkts, 0), "Length");
errr("stats-empty", cat(0x18), "Truncated");
errr("autoadd-config-short", cat(0x19, 0x1f), "Truncated");
errr("allowed-repeat-freq-partial", cat(0x1a, freqs.subarray(0, 7)), "Length");
errr("raw-data-short", cat(0x84, 0x28, 0xb5), "Truncated");
errr("login-success-mid", cat(loginLegacy, 1, 2), "Truncated");
errr("login-success-trailing", cat(loginNew, 0), "Length");
errr("login-fail-trailing", cat(0x86, 0, K2.subarray(0, 6), 0), "Length");
errr("status-response-short", cat(0x87, 0, K2.subarray(0, 5)), "Truncated");
errr("trace-data-short", traceData.subarray(0, traceData.length - 1), "Truncated");
errr("trace-data-trailing", cat(traceData, 0), "Length");
errr("binary-response-short", cat(0x8c, 0, 1, 2), "Truncated");
errr("path-discovery-bad-out", cat(0x8d, 0, K2.subarray(0, 6), 0xc1, 0x11, 0), "Value");
errr("path-discovery-no-in", cat(0x8d, 0, K2.subarray(0, 6), 1, 0x11), "Truncated");
errr("control-data-short", cat(0x8e, 0x28, 0xb5), "Truncated");
errr("contact-deleted-short", cat(0x8f, K2.subarray(0, 31)), "Truncated");
errr("contacts-full-trailing", cat(0x90, 0), "Length");

// ---------------------------------------------------------------- device side (§10, ADR-0002)
// Each case follows a branch of MyMesh::handleCmdFrame (M:line in the name). Bytes read past the
// end of the frame are 0 (the C++ reads the rest of cmd_frame there, §10).
const devices = [];
const devOk = (name, bytes, canonical) => devices.push({ name, bytes, expect: "ok", canonical });
const devErr = (name, bytes, code) => devices.push({ name, bytes, expect: `err:${code}` });
const devIgnored = (name, bytes) => devices.push({ name, bytes, expect: "ignored" });

devIgnored("empty (never handed to the C++, §1)", Buffer.alloc(0));
devIgnored("177-bytes (beyond MAX_FRAME_SIZE, §1)", cat(0x0a, zeros(176)));
devErr("code-0 M:2059", cat(0x00), 1);
devErr("reserved-44 M:52", cat(0x2c), 1);
devErr("reserved-53 M:2059", cat(0x35, 0), 1);
devErr("code-67 M:2059", cat(0x43), 1);
devErr("code-0xff M:2059", cat(0xff, 1, 2), 1);
// DEVICE_QUERY, APP_START, RUN_CLI_COMMAND, SEND_TXT_MSG
devErr("device-query-short M:1056", cat(0x16), 1);
devOk("device-query-trailing M:1056", cat(0x16, 3, 0xaa), cat(0x16, 3));
devErr("app-start-7 M:1076", cat(0x01, zeros(6)), 1);
devOk("app-start-8 M:1076", cat(0x01, zeros(7)), cat(0x01, zeros(7)));
devErr("run-cli-2 M:1118", cat(0x42, Buffer.from("v")), 1);
devOk("run-cli-3 M:1118", cat(0x42, Buffer.from("ve")), cat(0x42, Buffer.from("ve")));
devErr("txt-msg-13 M:1132", cat(0x02, 0, 0, u32(T), K1.subarray(0, 6)), 1);
devOk("txt-msg-14 M:1132", cat(0x02, 0, 0, u32(T), K1.subarray(0, 6), 0x41), cat(0x02, 0, 0, u32(T), K1.subarray(0, 6), 0x41));
// SEND_CHANNEL_TXT_MSG: no length check; below 7 bytes, a negative text length is passed to
// memcpy (BCM:528), undefined behaviour: rejected (ERR 1).
devOk("channel-txt-7 M:1178", cat(0x03, 0, 1, u32(T)), cat(0x03, 0, 1, u32(T)));
devErr("channel-txt-6 M:1178 BCM:528", cat(0x03, 0, 1, 0, 0, 0), 1);
devErr("channel-txt-1 M:1178 BCM:528", cat(0x03), 1);
// SEND_CHANNEL_DATA: len < 4 → ERR 6; invalid path_len → ERR 6; path and data_type read without
// a check, payload = the remainder.
devErr("channel-data-3 M:1199-1202", cat(0x3e, 1, 0xff), 6);
devErr("channel-data-hash-size-4 M:1208-1211", cat(0x3e, 1, 0xc1, 0, 0, 0), 6);
devErr("channel-data-path-66 M:1208-1211", cat(0x3e, 1, 0x61, zeros(10)), 6);
devOk("channel-data-type-1-byte M:1220-1223", cat(0x3e, 1, 0xff, 0x4c), cat(0x3e, 1, 0xff, 0x4c, 0x00));
devOk("channel-data-no-type M:1220-1223", cat(0x3e, 1, 0x01, 0xaa), cat(0x3e, 1, 0x01, 0xaa, 0, 0));
devOk("channel-data-path-overrun M:1216-1223", cat(0x3e, 1, 0x02, 0xaa), cat(0x3e, 1, 0x02, 0, 0, 0, 0));
devOk("channel-data-type-0 M:1228", cat(0x3e, 1, 0xff, u16(0), 0xaa), cat(0x3e, 1, 0xff, u16(0), 0xaa));
// GET_CONTACTS: since only if len ≥ 5.
devOk("get-contacts-partial-since M:1242-1246", cat(0x04, 1, 2), cat(0x04));
devOk("get-contacts-trailing M:1242", cat(0x04, u32(5), 9), cat(0x04, u32(5)));
// SET_ADVERT_NAME, SET_ADVERT_LATLON
devErr("set-name-empty M:1259", cat(0x08), 1);
devOk("set-name-long M:1259", cat(0x08, Buffer.alloc(40, 0x61)), cat(0x08, Buffer.alloc(40, 0x61)));
devErr("latlon-8 M:1266", cat(0x0e, i32(1), 0, 0, 0), 1);
devOk("latlon-partial-alt M:1270", cat(latlon, 1, 2), latlon);
devOk("latlon-alt-trailing M:1270", cat(latlon, i32(-12), 7), cat(latlon, i32(-12)));
// GET_DEVICE_TIME, SET_DEVICE_TIME, SEND_SELF_ADVERT
devOk("get-time-trailing M:1281", cat(0x05, 1, 2), cat(0x05));
devErr("set-time-4 M:1287", cat(0x06, 1, 2, 3), 1);
devOk("set-time-trailing M:1287", cat(0x06, u32(T), 0), cat(0x06, u32(T)));
devOk("advert-no-mode M:1305", cat(0x07), cat(0x07));
devOk("advert-mode-2 M:1305 (zero hop)", cat(0x07, 2), cat(0x07, 0));
devOk("advert-trailing M:1305", cat(0x07, 1, 5), cat(0x07, 1));
// RESET_PATH, ADD_UPDATE_CONTACT (fields read up to [135], M:194-217)
devErr("reset-path-32 M:1317", cat(0x0d, K1.subarray(0, 31)), 1);
devOk("reset-path-trailing M:1317", cat(0x0d, K1, 0), cat(0x0d, K1));
devErr("add-contact-35 M:1328", add.subarray(0, 35), 1);
devOk("add-contact-36 M:1328 M:194-206", add.subarray(0, 36), cat(add.subarray(0, 36), zeros(100)));
devOk("add-contact-100 M:194-206", add.subarray(0, 100), cat(add.subarray(0, 100), zeros(36)));
devOk("add-contact-140 M:207", cat(add, 1, 2, 3, 4), add);
devOk("add-contact-146 M:211", cat(addLatLon, 1, 2), addLatLon);
devOk("add-contact-150 M:211", cat(addLastmod, 1, 2), addLastmod);
// REMOVE_CONTACT, SHARE_CONTACT, GET_CONTACT_BY_KEY: key read without a length check.
devOk("remove-contact-short M:1349", cat(0x0f, K2.subarray(0, 4)), cat(0x0f, K2.subarray(0, 4), zeros(28)));
devOk("remove-contact-alone M:1349", cat(0x0f), cat(0x0f, zeros(32)));
devOk("share-contact-trailing M:1359", cat(0x10, K2, 1), cat(0x10, K2));
devOk("get-contact-by-key-short M:1371", cat(0x1e, K2.subarray(0, 31)), cat(0x1e, K2.subarray(0, 31), 0));
// EXPORT_CONTACT, IMPORT_CONTACT, SYNC_NEXT_MESSAGE
devOk("export-contact-partial M:1380", cat(0x11, K2.subarray(0, 10)), cat(0x11));
devOk("export-contact-trailing M:1380", cat(0x11, K2, 1, 2), cat(0x11, K2));
devErr("import-contact-98 M:1409", cat(0x12, ADVERT_RAW.subarray(0, 97)), 1);
devOk("import-contact-99 M:1409", cat(0x12, ADVERT_RAW.subarray(0, 98)), cat(0x12, ADVERT_RAW.subarray(0, 98)));
devOk("sync-trailing M:1415", cat(0x0a, 0), cat(0x0a));
// SET_RADIO_PARAMS, SET_RADIO_TX_POWER, SET_TUNING_PARAMS: no length check.
devOk("radio-alone M:1424", cat(0x0b), cat(0x0b, zeros(10)));
devOk("radio-partial M:1424", cat(0x0b, u32(869618)), cat(0x0b, u32(869618), zeros(6)));
devOk("radio-repeat M:1435", cat(radio, 1), cat(radio, 1));
devOk("radio-repeat-trailing M:1435", cat(radio, 1, 9), cat(radio, 1));
devOk("tx-power-alone M:1460", cat(0x0c), cat(0x0c, 0));
devOk("tx-power-trailing M:1460", cat(0x0c, 22, 0), cat(0x0c, 22));
devOk("tuning-partial M:1470", cat(0x15, u32(5)), cat(0x15, u32(5), u32(0)));
devOk("get-tuning-trailing M:1481", cat(0x2b, 1), cat(0x2b));
// SET_OTHER_PARAMS: [1] always read.
devOk("other-params-alone M:1489", cat(0x26), cat(0x26, 0));
devOk("other-params-trailing M:1490-1500", cat(0x26, 1, 2, 3, 4, 5, 6), cat(0x26, 1, 2, 3, 4));
// SET_PATH_HASH_MODE: [1] == 0 and len ≥ 3; mode ≥ 3 → ERR 6 after parsing.
devErr("path-hash-reserved-1 M:1504", cat(0x3d, 1, 1), 1);
devErr("path-hash-2-bytes M:1504", cat(0x3d, 0), 1);
devOk("path-hash-trailing M:1504", cat(0x3d, 0, 2, 9), cat(0x3d, 0, 2));
devOk("path-hash-mode-3 M:1505", cat(0x3d, 0, 3), cat(0x3d, 0, 3));
// REBOOT, FACTORY_RESET: memcmp without a length check.
devErr("reboot-short M:1512", cat(0x13, Buffer.from("reb")), 1);
devErr("reboot-wrong M:1512", cat(0x13, Buffer.from("rebooT")), 1);
devOk("reboot-trailing M:1512", cat(0x13, Buffer.from("reboot!")), cat(0x13, Buffer.from("reboot")));
devErr("factory-reset-short M:1965", cat(0x33, Buffer.from("res")), 1);
devOk("factory-reset-trailing M:1965", cat(0x33, Buffer.from("reset!")), cat(0x33, Buffer.from("reset")));
devOk("batt-trailing M:1517", cat(0x14, 0), cat(0x14));
devOk("export-key-trailing M:1528", cat(0x17, 0), cat(0x17));
devErr("import-key-64 M:1537", cat(0x18, seq(0, 63)), 1);
devOk("import-key-trailing M:1537", cat(0x18, seq(0x20, 64), 0), cat(0x18, seq(0x20, 64)));
// SEND_RAW_DATA: len ≥ 6; invalid path_len → ERR 1; payload < 4 → ERR 6.
devErr("raw-data-5 M:1557", cat(0x19, 0, 1, 2, 3), 1);
devErr("raw-data-flood M:1574", cat(0x19, 0xff, 1, 2, 3, 4), 1);
devErr("raw-data-short-payload M:1563", cat(0x19, 2, 0xaa, 0xbb, 1, 2, 3), 6);
devErr("raw-data-path-overrun M:1563", cat(0x19, 10, 1, 2, 3, 4), 6);
devOk("raw-data-ok M:1566", rawData, rawData);
// Requests to a contact: length preconditions.
devErr("login-32 M:1577", cat(0x1a, K2.subarray(0, 31)), 1);
devOk("login-no-password M:1577", cat(0x1a, K2), cat(0x1a, K2));
devErr("anon-req-33 M:1599", cat(0x39, K2), 1);
devOk("anon-req-34 M:1599", cat(0x39, K2, 7), cat(0x39, K2, 7));
devErr("status-req-32 M:1630", cat(0x1b, K2.subarray(0, 31)), 1);
devOk("status-req-trailing M:1630", cat(0x1b, K2, 1), cat(0x1b, K2));
devErr("path-discovery-reserved-1 M:1651", cat(0x34, 1, K2), 1);
devErr("path-discovery-33 M:1651", cat(0x34, 0, K2.subarray(0, 31)), 1);
devOk("path-discovery-trailing M:1651", cat(0x34, 0, K2, 1), cat(0x34, 0, K2));
devErr("telemetry-5 M:1680 M:1700", cat(0x27, 0, 0, 0, 0xaa), 1);
devErr("telemetry-35 M:1680", cat(0x27, 0, 0, 0, K2.subarray(0, 31)), 1);
devOk("telemetry-self-reserved M:1700", cat(0x27, 1, 2, 3), cat(0x27, 1, 2, 3));
devOk("telemetry-trailing M:1680", cat(0x27, 0, 0, 0, K2, 1), cat(0x27, 0, 0, 0, K2));
devErr("binary-req-33 M:1720", cat(0x32, K2), 1);
devOk("has-connection-trailing M:1741", cat(0x1c, K2, 0), cat(0x1c, K2));
devErr("logout-32 M:1748", cat(0x1d, K2.subarray(0, 31)), 1);
// GET_CHANNEL, SET_CHANNEL (32-byte key not supported)
devErr("get-channel-alone M:1752", cat(0x1f), 1);
devOk("get-channel-trailing M:1752", cat(0x1f, 3, 4), cat(0x1f, 3));
devErr("set-channel-66 M:1767", cat(0x20, 1, CH_NAME, seq(0, 32)), 1);
devErr("set-channel-49 M:1769", cat(0x20, 1, CH_NAME, CH_SECRET.subarray(0, 15)), 1);
devOk("set-channel-55 M:1769", cat(0x20, 1, CH_NAME, CH_SECRET, seq(0, 5)), cat(0x20, 1, CH_NAME, CH_SECRET));
// Signature
devOk("sign-start-trailing M:1781", cat(0x21, 0), cat(0x21));
devErr("sign-data-empty M:1793", cat(0x22), 1);
devOk("sign-finish-trailing M:1801", cat(0x23, 0), cat(0x23));
// SEND_TRACE_PATH: len > 10; hashes inconsistent with flags → ERR 6.
devErr("trace-10 M:1813", cat(0x24, u32(7), u32(9), 0), 1);
devErr("trace-odd M:1817", cat(0x24, u32(7), u32(9), 1, 1, 2, 3), 6);
devErr("trace-65-hops M:1817", cat(0x24, u32(7), u32(9), 0, seq(0, 65)), 6);
devOk("trace-64-hops M:1817", cat(0x24, u32(7), u32(9), 0, seq(0, 64)), cat(0x24, u32(7), u32(9), 0, seq(0, 64)));
devOk("trace-hash-8 M:1816", cat(0x24, u32(7), u32(9), 3, seq(0, 16)), cat(0x24, u32(7), u32(9), 3, seq(0, 16)));
// SET_DEVICE_PIN, variables, advert paths, stats
devErr("set-pin-4 M:1839", cat(0x25, 1, 2, 3), 1);
devOk("set-pin-trailing M:1839", cat(0x25, u32(0), 0), cat(0x25, u32(0)));
devOk("get-custom-vars-trailing M:1853", cat(0x28, 0), cat(0x28));
devErr("custom-var-3 M:1867", cat(0x29, Buffer.from("a:")), 1);
devOk("custom-var-no-colon M:1871 (ERR 6 after parsing)", cat(0x29, Buffer.from("abc")), cat(0x29, Buffer.from("abc")));
devErr("advert-path-33 M:1893", cat(0x2a, 0, K2.subarray(0, 31)), 1);
devOk("advert-path-trailing M:1893", cat(0x2a, 5, K2, 1), cat(0x2a, 5, K2));
devErr("stats-no-type M:1914", cat(0x38), 1);
devOk("stats-type-9 M:1963 (ERR 6 after parsing)", cat(0x38, 9), cat(0x38, 9));
devOk("stats-trailing M:1914", cat(0x38, 1, 0), cat(0x38, 1));
// SET_FLOOD_SCOPE_KEY: [1] == 0 (key if len ≥ 18) or [1] == 1.
devErr("scope-alone M:1978", cat(0x36), 1);
devErr("scope-2 M:1978 M:1986", cat(0x36, 2), 1);
devOk("scope-short-key M:1981-1982", cat(0x36, 0, seq(1, 10)), cat(0x36, 0));
devOk("scope-key-trailing M:1980", cat(0x36, 0, SCOPE_KEY, 1), cat(0x36, 0, SCOPE_KEY));
devOk("unscoped-trailing M:1986", cat(0x36, 1, 5), cat(0x36, 1));
// SET_DEFAULT_FLOOD_SCOPE: scope if len ≥ 48, otherwise clear.
devOk("default-scope-partial M:2000", cat(0x3f, fixed("fr", 31)), cat(0x3f));
devOk("default-scope-trailing M:1990", cat(0x3f, fixed("fr", 31), SCOPE_KEY, 1), cat(0x3f, fixed("fr", 31), SCOPE_KEY));
devOk("default-scope-no-nul M:1991 (ERR 6 after parsing)", cat(0x3f, Buffer.alloc(31, 0x61), SCOPE_KEY), cat(0x3f, Buffer.alloc(31, 0x61), SCOPE_KEY));
devOk("get-default-scope-trailing M:2006", cat(0x40, 0), cat(0x40));
// SEND_CONTROL_DATA: len ≥ 2 and [1] & 0x80.
devErr("control-alone M:2015", cat(0x37), 1);
devErr("control-no-flag M:2015", cat(0x37, 0x10, 1), 1);
devOk("control-flag-only M:2015", cat(0x37, 0x80), cat(0x37, 0x80));
// SET_AUTOADD_CONFIG: [1] always read, max_hops if len ≥ 3 (capped at 64 after parsing).
devOk("autoadd-alone M:2024", cat(0x3a), cat(0x3a, 0));
devOk("autoadd-trailing M:2025", cat(0x3a, 1, 2, 3), cat(0x3a, 1, 2));
devOk("autoadd-hops-200 M:2026", cat(0x3a, 1, 200), cat(0x3a, 1, 200));
devOk("get-autoadd-trailing M:2030", cat(0x3b, 0), cat(0x3b));
devOk("get-repeat-freq-trailing M:2036", cat(0x3c, 0), cat(0x3c));
// SEND_RAW_PACKET: len ≥ 4; invalid packet → ERR 6 after parsing (and after ERR 3).
devErr("raw-packet-3 M:2045", cat(0x41, 1, 0x11), 1);
devOk("raw-packet-4 M:2045", cat(0x41, 1, 0x11, 0), cat(0x41, 1, 0x11, 0));

// ---------------------------------------------------------------- carriers
const carriers = [];
const env156 = Buffer.from(Array.from({ length: 156 }, (_, i) => (i * 7) % 3 === 0 ? 0 : i));
carriers.push({ name: "dm", envelope: ENV, text: DM_TEXT });
carriers.push({ name: "dm-156", envelope: env156, text: cat(0xfe, cobs(env156)) });
assert.equal(cat(0xfe, cobs(env156)).length, 158);
carriers.push({ name: "dm-no-zero", envelope: seq(1, 40), text: cat(0xfe, cobs(seq(1, 40))) });
const notEnvelopes = [
  ["plain", Buffer.from("hello")],
  ["marker-only", cat(0xfe)],
  ["empty-envelope", cat(0xfe, 0x01)],
  ["zero-in-cobs", cat(0xfe, 0x03, 0x41, 0x00)],
  ["cobs-overrun", cat(0xfe, 0x05, 0x41)],
  ["too-long", cat(0xfe, cobs(Buffer.alloc(157, 0x41)))],
];
const channels = [
  { key: seq(0x00, 32) },
  { key: Buffer.from("0212235b07571ff3844b4ab36cbaa90bf88d4d670923cf7c6da73dfea4546307", "hex") },
].map(({ key }) => ({ key, name: `ln-${hex(key.subarray(16, 18))}`, secret: key.subarray(0, 16) }));
const scopes = [{ scope: "fr", key: SCOPE_KEY }, { scope: "eu", key: createHash("sha256").update("#eu").digest().subarray(0, 16) }];

// ---------------------------------------------------------------- output
const q = (s) => `"${String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
const out = ["# MeshCore companion protocol vectors, written from docs/spec/companion.md.",
  "# Generated by testdata/proto/gen.mjs, cross-checked with meshcore.js 1.15.0 (independent oracle).", ""];
for (const v of commands) out.push("[[command]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, 'expect = "ok"', `dump = ${q(v.dump)}`, "");
for (const v of commandErrs) out.push("[[command]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, `expect = "err:${v.kind}"`, "");
for (const v of responses) out.push("[[response]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, 'expect = "ok"', `dump = ${q(v.dump)}`, "");
for (const v of responseErrs) out.push("[[response]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, `expect = "err:${v.kind}"`, "");
for (const v of devices) {
  out.push("[[device]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, `expect = ${q(v.expect)}`);
  if (v.canonical) out.push(`canonical = ${q(hex(v.canonical))}`);
  out.push("");
}
for (const v of scopes) out.push("[[scope]]", `scope = ${q(v.scope)}`, `key = ${q(hex(v.key))}`, "");

// Coverage: every v1.18 code has at least one positive vector.
const reservedCmd = [44, 45, 46, 47, 48, 49, 53];
for (let c = 1; c <= 66; c++) {
  if (!reservedCmd.includes(c)) assert.ok(commands.some((v) => v.bytes[0] === c), `command ${c} has no vector`);
}
for (const c of [...Array(30).keys(), ...Array.from({ length: 17 }, (_, i) => 0x80 + i)]) {
  assert.ok(responses.some((v) => v.bytes[0] === c), `response ${c} has no vector`);
}

writeFileSync(process.argv[2], out.join("\n"));
const devOkN = devices.filter((v) => v.expect === "ok").length;
console.log(`${commands.length} commands, ${commandErrs.length} rejected; ${responses.length} responses, ${responseErrs.length} rejected; ` +
  `${devices.length} device-side cases (${devOkN} accepted, ${devices.length - devOkN} rejected); ` +
  `${carriers.length + notEnvelopes.length} carriers, ${channels.length} channels, ${scopes.length} scopes`);
console.log(`meshcore.js 1.15.0: ${jsEncoded} encodings and ${jsDecoded} decodings cross-checked`);
