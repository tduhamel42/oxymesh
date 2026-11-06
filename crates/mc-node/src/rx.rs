//! Reception (`Mesh::onRecvPacket`, `BaseChatMesh`; `docs/spec/air.md` §4).

use mc_air::crypto::{self, PUB_KEY_LEN, SECRET_LEN};
use mc_air::payload::{GroupDataPlain, GroupTxtPlain, PathPlain, Payload, TxtPlain};
use mc_air::{AppData, Packet, PacketBuf, PathLen, PayloadType};
use mc_core::{Duration, Instant};
use rand_core::Rng;

use crate::contacts::{Contact, PATH_UNKNOWN, Slot, TYPE_NONE};
use crate::node::{Event, Node, Text};

/// Delay before acknowledging a message (`TXT_ACK_DELAY`).
pub const TXT_ACK_DELAY: u64 = 200;

fn text_of(b: &[u8]) -> Text {
    let mut t = Text::new();
    let _ = t.extend_from_slice(b.get(..b.len().min(t.capacity())).unwrap_or(&[]));
    t
}

impl Node {
    /// A frame received by the radio.
    pub fn on_rx(&mut self, now: Instant, raw: &[u8], snr4: i8, rssi: i16, rng: &mut impl Rng) {
        self.last_snr4 = snr4;
        self.last_rssi = rssi;
        self.disp.stats.rx_airtime_ms += self.prefs.radio.airtime_ms(raw.len());
        let rssi8 = i8::try_from(rssi.clamp(-128, 127)).unwrap_or(-128);
        self.emit(Event::RxLog {
            snr4,
            rssi: rssi8,
            raw: text_of(raw),
        });
        let Ok(p) = Packet::decode(raw) else {
            self.disp.stats.recv_errors = self.disp.stats.recv_errors.wrapping_add(1);
            return;
        };
        let Ok(mut pkt) = PacketBuf::from_packet(&p) else {
            return;
        };
        if pkt.header.route.is_flood() {
            self.disp.stats.recv_flood = self.disp.stats.recv_flood.wrapping_add(1);
        } else {
            self.disp.stats.recv_direct = self.disp.stats.recv_direct.wrapping_add(1);
        }
        self.process(now, &mut pkt, snr4, rssi8, rng);
    }

    /// `IMPORT_CONTACT`: the advert frame is replayed as if received by flood
    /// (`BaseChatMesh.cpp:588-601`). False if it is not an advert.
    pub fn import_advert(&mut self, now: Instant, raw: &[u8], rng: &mut impl Rng) -> bool {
        let Ok(p) = Packet::decode(raw) else {
            return false;
        };
        if p.header.ptype != PayloadType::ADVERT {
            return false;
        }
        let Ok(mut pkt) = PacketBuf::from_packet(&p) else {
            return false;
        };
        pkt.header.route = mc_air::Route::Flood;
        pkt.transport = None;
        self.process(now, &mut pkt, 0, 0, rng);
        true
    }

    fn allow_forward(&self) -> bool {
        self.prefs.repeat
    }

    fn self_hash_matches(&self, h: &[u8]) -> bool {
        self.id.pub_key().starts_with(h)
    }

    fn process(
        &mut self,
        now: Instant,
        pkt: &mut PacketBuf,
        snr4: i8,
        rssi: i8,
        rng: &mut impl Rng,
    ) {
        let direct = pkt.header.route.is_direct();
        let ptype = pkt.header.ptype;

        // Direct TRACE (`Mesh.cpp:42-68`).
        if direct && ptype == PayloadType::TRACE {
            self.on_trace(now, pkt, snr4, rng);
            return;
        }
        // Zero-hop CONTROL only (`Mesh.cpp:70-76`).
        if direct
            && ptype == PayloadType::CONTROL
            && pkt.payload().first().is_some_and(|f| f & 0x80 != 0)
        {
            if pkt.path_len.hops() == 0 {
                let path_len = pkt.path_len.byte();
                let data = text_of(pkt.payload());
                self.emit(Event::ControlData {
                    snr4,
                    rssi,
                    path_len,
                    data,
                });
            }
            return;
        }
        // Direct with a path: forward only (`Mesh.cpp:78-110`).
        if direct && pkt.path_len.hops() > 0 {
            if ptype == PayloadType::ACK
                && let Some(crc) = pkt
                    .payload()
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .map(u32::from_le_bytes)
            {
                let _ = self.process_ack(now, crc);
            }
            let size = usize::from(pkt.path_len.hash_size());
            if self.self_hash_matches(pkt.path().get(..size).unwrap_or(&[0xFF; 4]))
                && self.allow_forward()
            {
                let d = self.retransmit_delay(pkt, self.prefs.direct_tx_delay_milli, rng);
                let fresh = self.seen.check_and_mark(&pkt.hash(), true);
                if fresh && pkt.pop_front_hop().is_ok() {
                    self.disp
                        .queue(pkt.clone(), 0, now + Duration::from_millis(d));
                }
            }
            return;
        }

        let Ok(pl) = Payload::parse(ptype, pkt.payload()) else {
            return;
        };
        let hash = pkt.hash();
        let mut do_not_retransmit = false;
        match pl {
            Payload::Ack { crc, .. } => {
                if !self.seen.check_and_mark(&hash, direct) {
                    return;
                }
                if let Some(key) = self.process_ack(now, crc) {
                    do_not_retransmit = true;
                    self.return_path_retry(now, &key, pkt);
                }
            }
            Payload::Addressed {
                ptype,
                dest,
                src,
                sealed,
            } => {
                if !self.seen.check_and_mark(&hash, direct) {
                    return;
                }
                if dest == self.id.hash() {
                    let sealed: heapless::Vec<u8, 184> =
                        heapless::Vec::from_slice(sealed).unwrap_or_default();
                    do_not_retransmit = self.on_addressed(now, pkt, ptype, src, &sealed, snr4, rng);
                }
            }
            Payload::AnonReq { dest, .. } => {
                if !self.seen.check_and_mark(&hash, direct) {
                    return;
                }
                // A companion does not serve anonymous requests; they are ignored after decryption
                // like in C++ (no `onAnonDataRecv` on the companion side).
                if dest == self.id.hash() {
                    do_not_retransmit = true;
                }
            }
            Payload::Group {
                ptype,
                channel,
                sealed,
            } => {
                if !self.seen.check_and_mark(&hash, direct) {
                    return;
                }
                let sealed: heapless::Vec<u8, 184> =
                    heapless::Vec::from_slice(sealed).unwrap_or_default();
                for idx in self.channels.by_hash(channel) {
                    let Some(ch) = self.channels.0.get(idx) else {
                        continue;
                    };
                    let mut buf = [0u8; 184];
                    if let Ok(plain) = crypto::open(&ch.secret.0, &sealed, &mut buf) {
                        // The app sees 0xFF for a direct (zero-hop) packet (`MyMesh.cpp:572, 617`).
                        let path_len = if direct { 0xFF } else { pkt.path_len.byte() };
                        self.on_group(ptype, idx, plain, snr4, path_len);
                        break;
                    }
                }
            }
            Payload::Advert(a) => {
                if a.pub_key == self.id.pub_key() {
                    return;
                }
                if !self.seen.check_and_mark(&hash, direct) {
                    return;
                }
                if !a.verify() {
                    return;
                }
                let key = *a.pub_key;
                let ts = a.ts;
                let app: heapless::Vec<u8, 32> =
                    heapless::Vec::from_slice(a.app_data).unwrap_or_default();
                self.on_advert(now, pkt, &key, ts, &app, snr4);
            }
            Payload::RawCustom(data) => {
                if direct && self.seen.check_and_mark(&hash, true) {
                    let data = text_of(data);
                    self.emit(Event::RawData { snr4, rssi, data });
                }
                return;
            }
            Payload::Multipart { inner, body, .. } => {
                if inner == PayloadType::ACK && body.len() >= 4 {
                    let h = mc_air::packet::packet_hash(PayloadType::ACK, pkt.path_len, body);
                    if self.seen.check_and_mark(&h, direct)
                        && let Some(crc) = body
                            .get(..4)
                            .and_then(|b| b.try_into().ok())
                            .map(u32::from_le_bytes)
                    {
                        let _ = self.process_ack(now, crc);
                    }
                }
                return;
            }
            Payload::Trace { .. } | Payload::Control { .. } | Payload::Unknown(_) => return,
        }
        // Flood forwarding (`routeRecvPacket`).
        if !do_not_retransmit && pkt.header.route.is_flood() && self.allow_forward() {
            let h = self.id.pub_key();
            let hs = usize::from(pkt.path_len.hash_size());
            let mut hop = [0u8; 3];
            if let (Some(src), Some(dst)) = (h.get(..hs), hop.get_mut(..hs)) {
                dst.copy_from_slice(src);
            }
            if pkt.push_hop(hop.get(..hs).unwrap_or(&[])).is_ok() {
                let d = self.retransmit_delay(pkt, self.prefs.tx_delay_milli, rng);
                let pri = pkt.path_len.hops();
                self.disp
                    .queue(pkt.clone(), pri, now + Duration::from_millis(d));
            }
        }
    }

    fn on_trace(&mut self, now: Instant, pkt: &mut PacketBuf, snr4: i8, rng: &mut impl Rng) {
        let raw_len = pkt.path_len.byte();
        if usize::from(raw_len) >= mc_air::packet::PATH_MAX {
            return;
        }
        let Ok(Payload::Trace {
            tag,
            auth,
            flags,
            hashes,
        }) = Payload::parse(PayloadType::TRACE, pkt.payload())
        else {
            return;
        };
        let sz = flags & 3;
        let offset = usize::from(raw_len) << sz;
        if offset >= hashes.len() {
            let mut snrs = heapless::Vec::new();
            let _ = snrs.extend_from_slice(pkt.path());
            let ev = Event::TraceData {
                path_len: raw_len,
                flags,
                tag,
                auth,
                hashes: text_of(hashes),
                snrs,
                final_snr4: snr4,
            };
            self.emit(ev);
            return;
        }
        let entry = hashes
            .get(offset..offset + (1usize << sz))
            .unwrap_or(&[0xFF; 9]);
        if self.self_hash_matches(entry)
            && self.allow_forward()
            && self.seen.check_and_mark(&pkt.hash(), true)
        {
            let snr = snr4.to_le_bytes();
            if pkt.push_hop(&snr).is_ok() {
                let d = self.retransmit_delay(pkt, self.prefs.direct_tx_delay_milli, rng);
                self.disp
                    .queue(pkt.clone(), 5, now + Duration::from_millis(d));
            }
        }
    }

    /// REQ, RESPONSE, TXT_MSG or PATH for us: try each contact with the matching hash.
    /// True if the frame was decrypted (it will not be forwarded).
    #[allow(clippy::too_many_arguments)]
    fn on_addressed(
        &mut self,
        now: Instant,
        pkt: &PacketBuf,
        ptype: PayloadType,
        src: u8,
        sealed: &[u8],
        snr4: i8,
        rng: &mut impl Rng,
    ) -> bool {
        for slot in self.contacts.by_hash(src) {
            let id = self.id.clone();
            let Some(secret) = self.contacts.get_mut(slot).and_then(|c| c.secret(&id)) else {
                continue;
            };
            let mut buf = [0u8; 184];
            let Ok(plain) = crypto::open(&secret, sealed, &mut buf) else {
                continue;
            };
            let plain: heapless::Vec<u8, 184> =
                heapless::Vec::from_slice(plain).unwrap_or_default();
            let Some(key) = self.contacts.get(slot).map(|c| c.pub_key) else {
                return true;
            };
            if ptype == PayloadType::PATH {
                let Ok(pp) = PathPlain::parse(&plain) else {
                    return true;
                };
                let out_len = pp.path_len;
                let out_path: heapless::Vec<u8, 64> =
                    heapless::Vec::from_slice(pp.path).unwrap_or_default();
                let extra: heapless::Vec<u8, 184> =
                    heapless::Vec::from_slice(pp.extra).unwrap_or_default();
                let reciprocal =
                    self.on_path(now, slot, pkt, out_len, &out_path, pp.extra_type, &extra);
                if reciprocal && pkt.header.route.is_flood() {
                    // Reciprocal path, sent direct along the learned path, after 500 ms.
                    let r4 = rng.next_u32().to_le_bytes();
                    if let Ok(rp) = mc_air::payload::path_return(
                        src,
                        self.id.hash(),
                        &secret,
                        pkt.path_len,
                        pkt.path(),
                        None,
                        r4,
                    ) {
                        self.send_direct(rp, out_len.byte(), &out_path, 500, now);
                    }
                }
            } else {
                self.on_peer_data(now, pkt, ptype, slot, &key, &secret, &plain, snr4, rng);
            }
            return true;
        }
        false
    }

    #[allow(clippy::too_many_arguments)]
    fn on_peer_data(
        &mut self,
        now: Instant,
        pkt: &PacketBuf,
        ptype: PayloadType,
        slot: Slot,
        key: &[u8; PUB_KEY_LEN],
        secret: &[u8; SECRET_LEN],
        plain: &[u8],
        snr4: i8,
        rng: &mut impl Rng,
    ) {
        let flood = pkt.header.route.is_flood();
        let path_len = if flood {
            pkt.path_len.byte()
        } else {
            PATH_UNKNOWN
        };
        match ptype {
            PayloadType::TXT_MSG => {
                if plain.len() <= 5 {
                    return;
                }
                let Ok(t) = TxtPlain::parse(plain) else {
                    return;
                };
                let epoch = self.rtc.now(now);
                match t.txt_type() {
                    0 => {
                        if let Some(c) = self.contacts.get_mut(slot) {
                            c.lastmod = epoch;
                        }
                        self.emit(Event::ContactMsg {
                            key: *key,
                            snr4,
                            path_len,
                            txt_type: 0,
                            ts: t.ts,
                            author: None,
                            text: text_of(t.text),
                        });
                        let crc = t.ack_crc(key);
                        let mut ack = [0u8; 6];
                        ack[..4].copy_from_slice(&crc.to_le_bytes());
                        ack[4] = t.after_nul;
                        ack[5] = (rng.next_u32() & 0xFF) as u8;
                        self.reply_ack(now, pkt, slot, secret, &ack);
                    }
                    1 => self.emit(Event::ContactMsg {
                        key: *key,
                        snr4,
                        path_len,
                        txt_type: 1,
                        ts: t.ts,
                        author: None,
                        text: text_of(t.text),
                    }),
                    3 => self.emit(Event::ContactMsg {
                        key: *key,
                        snr4,
                        path_len,
                        txt_type: 3,
                        ts: t.ts,
                        author: None,
                        text: text_of(t.text),
                    }),
                    2 => {
                        // Signed room-server message: `author prefix(4) ‖ text`.
                        let Some(rest) = plain.get(5..) else { return };
                        let author: Option<[u8; 4]> = rest.get(..4).and_then(|a| a.try_into().ok());
                        let body = rest.get(4..).unwrap_or(&[]);
                        let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
                        let text = body.get(..end).unwrap_or(&[]);
                        if let Some(c) = self.contacts.get_mut(slot) {
                            if t.ts > c.sync_since {
                                c.sync_since = t.ts;
                            }
                            c.lastmod = epoch;
                        }
                        self.emit(Event::ContactMsg {
                            key: *key,
                            snr4,
                            path_len,
                            txt_type: 2,
                            ts: t.ts,
                            author,
                            text: text_of(text),
                        });
                        let crc: [u8; 4] = crypto::sha256_trunc(&[
                            plain.get(..9 + end).unwrap_or(&[]),
                            self.id.pub_key(),
                        ]);
                        self.reply_ack(now, pkt, slot, secret, &crc);
                    }
                    _ => {}
                }
            }
            PayloadType::REQ => {
                if plain.len() <= 4 {
                    return;
                }
                let ts = u32::from_le_bytes(
                    plain
                        .get(..4)
                        .and_then(|b| b.try_into().ok())
                        .unwrap_or([0; 4]),
                );
                let mut path = heapless::Vec::new();
                let _ = path.extend_from_slice(pkt.path());
                let data = text_of(plain.get(4..).unwrap_or(&[]));
                self.emit(Event::Request {
                    key: *key,
                    ts,
                    data,
                    flood,
                    path_len: pkt.path_len.byte(),
                    path,
                });
            }
            PayloadType::RESPONSE => {
                if plain.is_empty() {
                    return;
                }
                self.emit(Event::Response {
                    key: *key,
                    data: text_of(plain),
                });
                self.return_path_retry(now, key, pkt);
            }
            _ => {}
        }
    }

    /// Message acknowledgement: a flood return PATH if the frame arrived by flood, otherwise
    /// `sendAckTo` (`BaseChatMesh.cpp:244-256`).
    fn reply_ack(
        &mut self,
        now: Instant,
        pkt: &PacketBuf,
        slot: Slot,
        secret: &[u8; SECRET_LEN],
        ack: &[u8],
    ) {
        let Some(c) = self.contacts.get(slot) else {
            return;
        };
        let dest = c.hash();
        let (out_len, out_path): (u8, heapless::Vec<u8, 64>) = (
            c.out_path_len,
            heapless::Vec::from_slice(c.out_path()).unwrap_or_default(),
        );
        if pkt.header.route.is_flood() {
            if let Ok(rp) = mc_air::payload::path_return(
                dest,
                self.id.hash(),
                secret,
                pkt.path_len,
                pkt.path(),
                Some((PayloadType::ACK.value(), ack)),
                [0; 4],
            ) {
                self.send_flood_scoped(rp, TXT_ACK_DELAY, now);
            }
        } else {
            self.send_ack_to(now, out_len, &out_path, ack);
        }
    }

    /// `sendAckTo`: by flood if the path is unknown, otherwise direct (with an extra
    /// MULTIPART-ACK if `multi_acks`).
    pub(crate) fn send_ack_to(&mut self, now: Instant, out_len: u8, out_path: &[u8], ack: &[u8]) {
        if out_len == PATH_UNKNOWN {
            if let Ok(a) = mc_air::payload::ack(ack) {
                self.send_flood_scoped(a, TXT_ACK_DELAY, now);
            }
            return;
        }
        let mut d = TXT_ACK_DELAY;
        if self.prefs.multi_acks > 0 {
            if let Ok(a1) = mc_air::payload::multi_ack(ack, 1) {
                self.send_direct(a1, out_len, out_path, d, now);
            }
            d += 300;
        }
        if let Ok(a2) = mc_air::payload::ack(ack) {
            self.send_direct(a2, out_len, out_path, d, now);
        }
    }

    /// Ack or response that arrived by flood while a direct path is known: send that path back
    /// direct after 3 s (`handleReturnPathRetry`).
    fn return_path_retry(&mut self, now: Instant, key: &[u8; PUB_KEY_LEN], pkt: &PacketBuf) {
        if !pkt.header.route.is_flood() {
            return;
        }
        let id = self.id.clone();
        let Some(slot) = self.contacts.find(key) else {
            return;
        };
        let Some(c) = self.contacts.get_mut(slot) else {
            return;
        };
        if c.out_path_len == PATH_UNKNOWN {
            return;
        }
        let Some(secret) = c.secret(&id) else { return };
        let dest = c.hash();
        let out_len = c.out_path_len;
        let out_path: heapless::Vec<u8, 64> =
            heapless::Vec::from_slice(c.out_path()).unwrap_or_default();
        if let Ok(rp) = mc_air::payload::path_return(
            dest,
            id.hash(),
            &secret,
            pkt.path_len,
            pkt.path(),
            None,
            [0; 4],
        ) {
            self.send_direct(rp, out_len, &out_path, 3000, now);
        }
    }

    /// Path received (`onContactPathRecv`); false if no reciprocal path must be sent.
    #[allow(clippy::too_many_arguments)]
    fn on_path(
        &mut self,
        now: Instant,
        slot: Slot,
        pkt: &PacketBuf,
        out_len: PathLen,
        out_path: &[u8],
        extra_type: u8,
        extra: &[u8],
    ) -> bool {
        // Response to a path discovery: reported, neither stored nor sent back.
        if extra_type == PayloadType::RESPONSE.value() && extra.len() > 4 {
            let tag = extra
                .get(..4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes);
            if self.pending_discovery != 0 && tag == Some(self.pending_discovery) {
                self.pending_discovery = 0;
                if let Some(key) = self.contacts.get(slot).map(|c| c.pub_key) {
                    let mut op = heapless::Vec::new();
                    let _ = op.extend_from_slice(out_path);
                    let mut ip = heapless::Vec::new();
                    let _ = ip.extend_from_slice(pkt.path());
                    self.emit(Event::PathDiscovery {
                        key,
                        out_len: out_len.byte(),
                        out_path: op,
                        in_len: pkt.path_len.byte(),
                        in_path: ip,
                    });
                }
                return false;
            }
        }
        let epoch = self.rtc.now(now);
        let Some(c) = self.contacts.get_mut(slot) else {
            return true;
        };
        c.out_path_len = out_len.byte();
        c.out_path = [0; 64];
        if let Some(dst) = c.out_path.get_mut(..out_path.len()) {
            dst.copy_from_slice(out_path);
        }
        c.lastmod = epoch;
        let key = c.pub_key;
        self.emit(Event::PathUpdated { key });
        if extra_type == PayloadType::ACK.value() && extra.len() >= 4 {
            if let Some(crc) = extra
                .get(..4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
            {
                let _ = self.process_ack(now, crc);
            }
        } else if extra_type == PayloadType::RESPONSE.value() && !extra.is_empty() {
            self.emit(Event::Response {
                key,
                data: text_of(extra),
            });
        }
        true
    }

    /// Looks up the ack in the table; if found, sends a `SEND_CONFIRMED` notification and clears
    /// it (`MyMesh::processAck`). Returns the contact's key.
    pub(crate) fn process_ack(&mut self, now: Instant, crc: u32) -> Option<[u8; PUB_KEY_LEN]> {
        for slot in self.acks.iter_mut() {
            if let Some(e) = slot
                && e.ack == crc
            {
                let rtt = now.saturating_duration_since(e.sent).as_millis();
                let key = e.key;
                *slot = None;
                self.emit(Event::SendConfirmed {
                    ack: crc.to_le_bytes(),
                    rtt_ms: u32::try_from(rtt).unwrap_or(u32::MAX),
                });
                return Some(key);
            }
        }
        None
    }

    fn on_group(&mut self, ptype: PayloadType, idx: usize, plain: &[u8], snr4: i8, path_len: u8) {
        let idx = u8::try_from(idx).unwrap_or(0);
        if ptype == PayloadType::GRP_TXT {
            if let Ok(g) = GroupTxtPlain::parse(plain) {
                self.emit(Event::ChannelMsg {
                    idx,
                    snr4,
                    path_len,
                    txt_type: g.txt_type,
                    ts: g.ts,
                    text: text_of(g.text),
                });
            }
        } else if let Ok(g) = GroupDataPlain::parse(plain) {
            self.emit(Event::ChannelData {
                idx,
                snr4,
                path_len,
                data_type: g.data_type,
                data: text_of(g.data),
            });
        }
    }

    /// Valid advert (`BaseChatMesh::onAdvertRecv`, companion add rules).
    fn on_advert(
        &mut self,
        now: Instant,
        pkt: &PacketBuf,
        key: &[u8; PUB_KEY_LEN],
        ts: u32,
        app: &[u8],
        snr4: i8,
    ) {
        let Ok(ad) = AppData::decode(app) else { return };
        if ad.name.is_none() {
            return;
        }
        let mut path = heapless::Vec::new();
        let _ = path.extend_from_slice(pkt.path());
        let mut name = [0u8; 32];
        let n = ad.name.unwrap_or(&[]);
        let n = n.get(..n.len().min(31)).unwrap_or(&[]);
        if let Some(dst) = name.get_mut(..n.len()) {
            dst.copy_from_slice(n);
        }
        self.emit(Event::AdvertHeard {
            key: *key,
            kind: ad.kind,
            path_len: pkt.path_len.byte(),
            path,
            snr4,
            name,
            location: ad.location,
        });
        let epoch = self.rtc.now(now);
        let mut slot = self.contacts.find(key);
        if let Some(s) = slot
            && let Some(c) = self.contacts.get(s)
        {
            if ts <= c.last_advert {
                return; // replay
            }
            if c.kind == TYPE_NONE
                && let Slot::Anon(i) = s
                && let Some(a) = self.contacts.anon.get_mut(i)
            {
                *a = None; // a temporary contact becomes a real contact
                slot = None;
            }
        }
        // Stored frame: FLOOD route, transport codes stripped (`BaseChatMesh.cpp:141-149`).
        let raw = {
            let mut b = pkt.clone();
            b.header.route = mc_air::Route::Flood;
            b.transport = None;
            let mut out = [0u8; 255];
            let n = b.encode(&mut out).unwrap_or(0);
            text_of(out.get(..n).unwrap_or(&[]))
        };
        let fill = |c: &mut Contact| {
            c.set_name(ad.name_str());
            c.kind = ad.kind;
            if let Some((la, lo)) = ad.location {
                c.lat = la;
                c.lon = lo;
            }
            c.last_advert = ts;
            c.lastmod = epoch;
        };
        if let Some(s) = slot {
            if let Some(c) = self.contacts.get_mut(s) {
                fill(c);
            }
            self.emit(Event::Advert {
                key: *key,
                raw: raw.clone(),
            });
            return;
        }
        let mut fresh = Contact::new(*key);
        fill(&mut fresh);
        let allowed = self.prefs.manual_add_contacts & 1 == 0 || {
            let bit = match ad.kind {
                1 => 0x02,
                2 => 0x04,
                3 => 0x08,
                4 => 0x10,
                _ => 0,
            };
            self.prefs.autoadd_config & bit != 0
        };
        let too_far =
            self.prefs.autoadd_max_hops > 0 && pkt.path_len.hops() >= self.prefs.autoadd_max_hops;
        if !allowed || too_far {
            self.emit(Event::NewAdvert { contact: fresh });
            return;
        }
        if self.contacts.list.is_full() {
            if self.prefs.autoadd_config & 0x01 != 0
                && let Some(i) = self.contacts.oldest_non_favourite()
                && let Some(c) = self.contacts.list.get_mut(i)
            {
                let old = c.pub_key;
                *c = fresh;
                self.emit(Event::ContactOverwritten { key: old });
                self.emit(Event::Advert {
                    key: *key,
                    raw: raw.clone(),
                });
                return;
            }
            self.emit(Event::NewAdvert { contact: fresh });
            self.emit(Event::ContactsFull);
            return;
        }
        let _ = self.contacts.list.push(fresh);
        self.emit(Event::Advert {
            key: *key,
            raw: raw.clone(),
        });
    }
}
