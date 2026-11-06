//! Requests to repeaters and servers (`BaseChatMesh.cpp:603-715`), traces, control and raw
//! data.

use mc_air::{PacketBuf, PathLen, PayloadType, Route, payload};
use mc_core::Instant;
use rand_core::Rng;

use crate::contacts::{Contact, PATH_UNKNOWN, Slot, TYPE_NONE};
use crate::node::{Node, SendError, Sent};

impl Node {
    /// `sendRequest`: `tag ‖ data` as REQ; tag = the device's unique time.
    pub fn send_request(
        &mut self,
        now: Instant,
        slot: Slot,
        data: &[u8],
    ) -> Result<Sent, SendError> {
        if data.len() > mc_air::packet::PAYLOAD_MAX - 16 {
            return Err(SendError::Failed);
        }
        let tag = self.rtc.unique(now);
        let mut plain = [0u8; 184];
        let n = 4 + data.len();
        let dst = plain.get_mut(..n).ok_or(SendError::Failed)?;
        let (t, d) = dst.split_at_mut(4);
        t.copy_from_slice(&tag.to_le_bytes());
        d.copy_from_slice(data);
        self.send_addressed(
            now,
            slot,
            PayloadType::REQ,
            plain.get(..n).unwrap_or(&[]),
            tag,
        )
    }

    /// `sendRequest(type)`: 13-byte request `tag, type, 0000, random(4)`.
    pub fn send_typed_request(
        &mut self,
        now: Instant,
        slot: Slot,
        req_type: u8,
        rng: &mut impl Rng,
    ) -> Result<Sent, SendError> {
        let tag = self.rtc.unique(now);
        let mut plain = [0u8; 13];
        plain[..4].copy_from_slice(&tag.to_le_bytes());
        plain[4] = req_type;
        plain[9..].copy_from_slice(&rng.next_u32().to_le_bytes());
        self.send_addressed(now, slot, PayloadType::REQ, &plain, tag)
    }

    fn send_addressed(
        &mut self,
        now: Instant,
        slot: Slot,
        ptype: PayloadType,
        plain: &[u8],
        tag: u32,
    ) -> Result<Sent, SendError> {
        let id = self.id.clone();
        let c = self.contacts.get_mut(slot).ok_or(SendError::NotFound)?;
        let secret = c.secret(&id).ok_or(SendError::Failed)?;
        let dest = c.hash();
        let pkt = payload::datagram(ptype, dest, id.hash(), &secret, plain)
            .map_err(|_| SendError::Failed)?;
        let (flood, timeout_ms) = self
            .send_to_contact(now, slot, pkt, 0)
            .ok_or(SendError::Failed)?;
        Ok(Sent {
            flood,
            ack: tag,
            timeout_ms,
        })
    }

    fn send_anon(
        &mut self,
        now: Instant,
        slot: Slot,
        plain: &[u8],
        tag: u32,
    ) -> Result<Sent, SendError> {
        let id = self.id.clone();
        let c = self.contacts.get_mut(slot).ok_or(SendError::NotFound)?;
        let secret = c.secret(&id).ok_or(SendError::Failed)?;
        let dest = c.hash();
        let pkt =
            payload::anon_datagram(dest, &id, &secret, plain).map_err(|_| SendError::Failed)?;
        let (flood, timeout_ms) = self
            .send_to_contact(now, slot, pkt, 0)
            .ok_or(SendError::Failed)?;
        Ok(Sent {
            flood,
            ack: tag,
            timeout_ms,
        })
    }

    /// `sendLogin`: `ts ‖ [sync_since if room server] ‖ password (at most 15 bytes)`, as
    /// ANON_REQ. The tag reported by the companion is the key prefix (`MyMesh.cpp:1577`).
    pub fn send_login(
        &mut self,
        now: Instant,
        slot: Slot,
        password: &[u8],
    ) -> Result<Sent, SendError> {
        let pw = crate::contacts::name_str(password);
        let pw = pw.get(..pw.len().min(15)).unwrap_or(&[]);
        let c = self.contacts.get(slot).ok_or(SendError::NotFound)?;
        let room = c.kind == mc_air::advert::TYPE_ROOM;
        let sync = c.sync_since;
        let ts = self.rtc.unique(now);
        let mut plain = [0u8; 24];
        let mut w = mc_core::Writer::new(&mut plain);
        w.put_u32_le(ts).map_err(|_| SendError::Failed)?;
        if room {
            w.put_u32_le(sync).map_err(|_| SendError::Failed)?;
        }
        w.put(pw).map_err(|_| SendError::Failed)?;
        let n = w.position();
        let key = self
            .contacts
            .get(slot)
            .map(|c| c.pub_key)
            .unwrap_or([0; 32]);
        let tag = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
        self.send_anon(now, slot, plain.get(..n).unwrap_or(&[]), tag)
    }

    /// `sendAnonReq`: `tag ‖ data` as ANON_REQ.
    pub fn send_anon_req(
        &mut self,
        now: Instant,
        slot: Slot,
        data: &[u8],
    ) -> Result<Sent, SendError> {
        let tag = self.rtc.unique(now);
        let mut plain = [0u8; 184];
        let n = 4 + data.len();
        let dst = plain.get_mut(..n).ok_or(SendError::Failed)?;
        let (t, d) = dst.split_at_mut(4);
        t.copy_from_slice(&tag.to_le_bytes());
        d.copy_from_slice(data);
        self.send_anon(now, slot, plain.get(..n).unwrap_or(&[]), tag)
    }

    /// Temporary contact for an anonymous request to an unknown key (v13+): type 0, zero-hop
    /// path (`MyMesh.cpp:1599-1629`).
    pub fn temp_contact(&mut self, now: Instant, key: &[u8; 32]) -> Slot {
        let i = self.contacts.anon_slot();
        let mut c = Contact::new(*key);
        c.kind = TYPE_NONE;
        c.out_path_len = 0;
        c.lastmod = self.rtc.now(now);
        if let Some(s) = self.contacts.anon.get_mut(i) {
            *s = Some(c);
        }
        Slot::Anon(i)
    }

    /// `SEND_TRACE_PATH`: direct TRACE, hashes at the end of the payload (`createTrace`).
    pub fn send_trace(
        &mut self,
        now: Instant,
        tag: u32,
        auth: u32,
        flags: u8,
        path: &[u8],
    ) -> Result<u32, SendError> {
        let mut b = PacketBuf::new(Route::Direct, PayloadType::TRACE);
        let mut head = [0u8; 9];
        head[..4].copy_from_slice(&tag.to_le_bytes());
        head[4..8].copy_from_slice(&auth.to_le_bytes());
        head[8] = flags;
        b.set_payload(&head).map_err(|_| SendError::Failed)?;
        // Reported timeout: direct, as many hops as entries (`calcDirectTimeoutMillisFor`).
        let entry = 1usize << (flags & 3);
        let hops = u8::try_from(path.len() / entry.max(1))
            .unwrap_or(63)
            .min(63);
        let t = self.prefs.radio.airtime_ms(2 + 9 + path.len());
        let timeout = 500 + (t * 6 + 250) * (u64::from(hops) + 1);
        let len = u8::try_from(path.len()).map_err(|_| SendError::Failed)?;
        self.send_direct(b, len, path, 0, now);
        Ok(u32::try_from(timeout).unwrap_or(u32::MAX))
    }

    /// `SEND_CONTROL_DATA`: zero-hop CONTROL.
    pub fn send_control(&mut self, now: Instant, data: &[u8]) -> Result<(), SendError> {
        let mut b = PacketBuf::new(Route::Direct, PayloadType::CONTROL);
        b.set_payload(data).map_err(|_| SendError::Failed)?;
        self.send_zero_hop(b, None, 0, now);
        Ok(())
    }

    /// `SEND_RAW_DATA`: direct RAW_CUSTOM along the given path.
    pub fn send_raw_data(
        &mut self,
        now: Instant,
        path_len: u8,
        path: &[u8],
        data: &[u8],
    ) -> Result<(), SendError> {
        if path_len == PATH_UNKNOWN || PathLen::from_byte(path_len).is_err() {
            return Err(SendError::Unsupported);
        }
        let mut b = PacketBuf::new(Route::Direct, PayloadType::RAW_CUSTOM);
        b.set_payload(data).map_err(|_| SendError::Failed)?;
        self.send_direct(b, path_len, path, 0, now);
        Ok(())
    }

    /// Replaces the identity (`IMPORT_PRIVATE_KEY`); shared secrets are recomputed.
    pub fn set_identity(&mut self, id: mc_air::LocalIdentity) {
        self.id = id;
        for c in self.contacts.list.iter_mut() {
            c.forget_secret();
        }
        for c in self.contacts.anon.iter_mut().flatten() {
            c.forget_secret();
        }
    }
}
