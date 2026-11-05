// Vecteurs du protocole compagnon MeshCore (docs/spec/companion.md), écrits octet par octet depuis
// la spec (repris de loranet, testdata/meshcore).
// Oracle indépendant : chaque trame que meshcore.js sait produire ou lire est recoupée avec lui
// (encodage identique pour les commandes, champs décodés identiques pour les réponses).
//
// Usage : (cd testdata/proto && npm ci && node gen.mjs vectors.toml)
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
  // Même échappement que le Display Rust (escape_debug des caractères, \xNN des octets invalides).
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

// ---------------------------------------------------------------- COBS (Cheshire et Baker)
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

// ---------------------------------------------------------------- oracle meshcore.js
class Capture extends Connection {
  constructor() { super(); this.frames = []; }
  async sendToRadioFrame(data) { this.frames.push(Buffer.from(data)); }
}
async function jsEncode(fn) {
  const c = new Capture();
  await fn(c);
  assert.equal(c.frames.length, 1);
  return c.frames[0];
}
async function jsDecode(frame) {
  const c = new Capture();
  let got = null;
  const code = frame[0];
  c.on(code, (v) => { got = v; });
  // meshcore.js publie les messages V3 sous le code non V3.
  if (code === Constants.ResponseCodes.ContactMsgRecvV3) c.on(Constants.ResponseCodes.ContactMsgRecv, (v) => { got = v; });
  if (code === Constants.ResponseCodes.ChannelMsgRecvV3) c.on(Constants.ResponseCodes.ChannelMsgRecv, (v) => { got = v; });
  c.onFrameReceived(new Uint8Array(frame));
  // meshcore.js publie ses événements dans un setTimeout(0) : on laisse passer un tour.
  await new Promise((r) => setTimeout(r, 1));
  assert.ok(got, `meshcore.js n'a rien lu dans ${hex(frame)}`);
  return got;
}
const same = (a, b, what) => assert.equal(hex(a), hex(b), what);

// ---------------------------------------------------------------- données
const K1 = seq(0x10, 32);
const K2 = seq(0x80, 32);
const T = 1_793_000_000;
const ENV = cat(0x10, seq(0, 8), u16(1), Buffer.from([0x00, 0x41, 0x00, 0x00]), seq(0xe0, 8)); // enveloppe d'exemple, avec des 0x00
const DM_TEXT = cat(0xfe, cobs(ENV));
const CH_NAME = fixed("ln-abcd", 32);
const CH_SECRET = seq(0x40, 16);
const SCOPE_KEY = createHash("sha256").update("#fr").digest().subarray(0, 16);

const commands = [], commandErrs = [], responses = [], responseErrs = [];
const okc = (name, bytes, dump) => commands.push({ name, bytes, dump });
const okr = (name, bytes, dump) => responses.push({ name, bytes, dump });

// ---------------------------------------------------------------- commandes
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
// meshcore.js ajoute toujours lat et lon : même trame, plus 8 octets.
same(add, (await jsEncode((c) => c.sendCommandAddUpdateContact(K1, 1, 0, 0xff, zeros(64), `ln-${hex(K1.subarray(0, 4))}`, 0, 0, 0))).subarray(0, add.length), "ADD_UPDATE_CONTACT");
okc("reset-path", cat(0x0d, K1), `RESET_PATH key=${hex(K1)}`);
same(cat(0x0d, K1), await jsEncode((c) => c.sendCommandResetPath(K1)), "RESET_PATH");
okc("advert-flood", cat(0x07, 1), "SEND_SELF_ADVERT flood=true");
same(cat(0x07, 1), await jsEncode((c) => c.sendCommandSendSelfAdvert(1)), "SEND_SELF_ADVERT");
okc("advert-zero-hop", cat(0x07, 0), "SEND_SELF_ADVERT flood=false");
const txt = cat(0x02, 0, 2, u32(T), K1.subarray(0, 6), DM_TEXT);
okc("txt-msg", txt, `SEND_TXT_MSG type=0 attempt=2 ts=${T} prefix=${hex(K1.subarray(0, 6))} text=${hex(DM_TEXT)}`);
// meshcore.js écrit le texte comme une chaîne : on ne recoupe qu'un texte ASCII.
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
okc("import-private-key", ikey, "IMPORT_PRIVATE_KEY <64 octets>");
same(ikey, await jsEncode((c) => c.sendCommandImportPrivateKey(seq(0x20, 64))), "IMPORT_PRIVATE_KEY");
const rawPkt = cat(0x11, 0x00, seq(0, 10));
okc("send-raw-packet", cat(0x41, 1, rawPkt), `SEND_RAW_PACKET prio=1 raw=${hex(rawPkt)}`);

const cdirect = cat(0x3e, 1, 0, u16(0xff4c), 0xaa);
okc("channel-data-zero-hop", cdirect, "SEND_CHANNEL_DATA 1 type=ff4c data=aa path=00:");
same(cdirect, await jsEncode((c) => c.sendCommandSendChannelData(1, 0, new Uint8Array(0), 0xff4c, Buffer.from([0xaa]))), "SEND_CHANNEL_DATA zéro saut");
const cpath = cat(0x3e, 2, 0x42, 0x11, 0x22, 0x33, 0x44, u16(0xff00), 1, 2);
okc("channel-data-path", cpath, "SEND_CHANNEL_DATA 2 type=ff00 data=0102 path=42:11223344");
okc("remove-contact", cat(0x0f, K2), `REMOVE_CONTACT key=${hex(K2)}`);
same(cat(0x0f, K2), await jsEncode((c) => c.sendCommandRemoveContact(K2)), "REMOVE_CONTACT");

const errc = (name, bytes, kind) => commandErrs.push({ name, bytes, kind });
errc("send-raw-packet-short", cat(0x41, 1, 0x11), "Truncated");
errc("import-private-key-short", cat(0x18, seq(0, 63)), "Truncated");
errc("get-device-time-trailing", cat(0x05, 0), "Length");
errc("empty", Buffer.alloc(0), "Truncated");
errc("unknown", cat(0x99), "Code");
errc("txt-no-text", cat(0x02, 0, 0, u32(T), K1.subarray(0, 6)), "Truncated");
errc("advert-mode-2", cat(0x07, 2), "Value");
errc("advert-no-mode", cat(0x07), "Truncated");
errc("channel-data-hash-size-4", cat(0x3e, 1, 0xc1, 0, 0, 0, 0, u16(0xff4c), 0xaa), "Value");
errc("channel-data-path-short", cat(0x3e, 1, 0x02, 0x11), "Truncated");
errc("channel-data-type-0", cat(0x3e, 1, 0xff, u16(0), 0xaa), "Value");
errc("channel-data-empty", cat(0x3e, 1, 0xff, u16(0xff4c)), "Truncated");
errc("device-query-trailing", cat(0x16, 3, 0), "Length");
errc("add-contact-short", cat(0x09, K1.subarray(0, 10)), "Truncated");
errc("add-contact-lat-lon", cat(add, i32(0), i32(0)), "Length");
errc("set-scope-short", cat(0x3f, fixed("fr", 31)), "Truncated");
errc("too-long", cat(0x02, 0, 0, u32(T), K1.subarray(0, 6), Buffer.alloc(164, 0x41)), "Length");

// ---------------------------------------------------------------- réponses
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
okr("private-key", pkey, "PRIVATE_KEY <64 octets>");
assert.equal(hex((await jsDecode(pkey)).privateKey), hex(seq(0x20, 64)));
okr("disabled", cat(0x0f), "DISABLED");
await jsDecode(cat(0x0f));
okr("other-unknown", cat(0x99, 1, 2), "OTHER code=0x99 body=0102");

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

// ---------------------------------------------------------------- porteurs
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

// ---------------------------------------------------------------- écriture
const q = (s) => `"${String(s).replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
const out = ["# Vecteurs du protocole compagnon MeshCore, écrits depuis docs/spec/companion.md.",
  "# Générés par testdata/proto/gen.mjs, recoupés avec meshcore.js 1.15.0 (oracle indépendant).", ""];
for (const v of commands) out.push("[[command]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, 'expect = "ok"', `dump = ${q(v.dump)}`, "");
for (const v of commandErrs) out.push("[[command]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, `expect = "err:${v.kind}"`, "");
for (const v of responses) out.push("[[response]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, 'expect = "ok"', `dump = ${q(v.dump)}`, "");
for (const v of responseErrs) out.push("[[response]]", `name = ${q(v.name)}`, `hex = ${q(hex(v.bytes))}`, `expect = "err:${v.kind}"`, "");
for (const v of scopes) out.push("[[scope]]", `scope = ${q(v.scope)}`, `key = ${q(hex(v.key))}`, "");
writeFileSync(process.argv[2], out.join("\n"));
console.log(`${commands.length} commandes, ${commandErrs.length} refus ; ${responses.length} réponses, ${responseErrs.length} refus ; ` +
  `${carriers.length + notEnvelopes.length} porteurs, ${channels.length} canaux, ${scopes.length} portées`);
