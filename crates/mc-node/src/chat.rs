//! Sending: direct messages, channels, adverts, acks (`BaseChatMesh.cpp`, `MyMesh.cpp`).

use mc_air::crypto::PUB_KEY_LEN;
use mc_air::{AppData, PacketBuf, PathLen, PayloadType, payload};
use mc_core::Instant;

use crate::contacts::{PATH_UNKNOWN, Slot, name_str};
use crate::node::{ExpectedAck, Node, SendError, Sent};

/// `SEND_TIMEOUT_BASE_MILLIS`, `FLOOD_SEND_TIMEOUT_FACTOR`, `DIRECT_SEND_PERHOP_FACTOR`,
/// `DIRECT_SEND_PERHOP_EXTRA_MILLIS` (`MyMesh.cpp:888-896`).
const TIMEOUT_BASE: u64 = 500;
const FLOOD_FACTOR: u64 = 16;
const DIRECT_PER_HOP: u64 = 6;
const DIRECT_PER_HOP_EXTRA: u64 = 250;
/// `CLI_REPLY_DELAY_MILLIS` (`BaseChatMesh.cpp:12`).
const CLI_REPLY_DELAY_MS: u64 = 600;

fn sat_u32(v: u64) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

impl Node {
    /// Send timeout reported in `SENT`: `t` = airtime of `2 + payload`
    /// (`getRawLength` before the route is set).
    fn send_timeout(&self, pkt: &PacketBuf, out_path_len: u8) -> u32 {
        let t = self.prefs.radio.airtime_ms(2 + pkt.payload().len());
        if out_path_len == PATH_UNKNOWN {
            sat_u32(TIMEOUT_BASE + FLOOD_FACTOR * t)
        } else {
            let hops = u64::from(out_path_len & 63);
            sat_u32(TIMEOUT_BASE + (t * DIRECT_PER_HOP + DIRECT_PER_HOP_EXTRA) * (hops + 1))
        }
    }

    /// Sends a datagram to a contact: direct if its path is known, otherwise flood within the
    /// scope. Returns (flood?, reported timeout).
    pub(crate) fn send_to_contact(
        &mut self,
        now: Instant,
        slot: Slot,
        pkt: PacketBuf,
        delay_ms: u64,
    ) -> Option<(bool, u32)> {
        let c = self.contacts.get(slot)?;
        let out_len = c.out_path_len;
        let out_path: heapless::Vec<u8, 64> =
            heapless::Vec::from_slice(c.out_path()).unwrap_or_default();
        let timeout = self.send_timeout(&pkt, out_len);
        if out_len == PATH_UNKNOWN {
            self.send_flood_scoped(pkt, delay_ms, now);
            Some((true, timeout))
        } else {
            self.send_direct(pkt, out_len, &out_path, delay_ms, now);
            Some((false, timeout))
        }
    }

    /// `SEND_TXT_MSG` (`MyMesh.cpp:1132-1177`). The text is cut at the first 0x00, like the
    /// firmware's C string. Types 1 and 3: device timestamp, no ack.
    #[allow(clippy::too_many_arguments)]
    pub fn send_text(
        &mut self,
        now: Instant,
        prefix: &[u8],
        txt_type: u8,
        attempt: u8,
        ts: u32,
        text: &[u8],
    ) -> Result<Sent, SendError> {
        let slot = self
            .contacts
            .find_prefix(prefix)
            .ok_or(SendError::NotFound)?;
        if !matches!(txt_type, 0 | 1 | 3) {
            return Err(SendError::Unsupported);
        }
        let text = name_str(text);
        let (ts, attempt_for_plain) = if txt_type == 0 {
            (ts, attempt)
        } else {
            (self.rtc.unique(now), attempt)
        };
        let id = self.id.clone();
        let c = self.contacts.get_mut(slot).ok_or(SendError::NotFound)?;
        let secret = c.secret(&id).ok_or(SendError::Failed)?;
        let dest = c.hash();
        let key = c.pub_key;
        let mut plain = [0u8; 184];
        let (n, crc) = if txt_type == 0 {
            payload::txt_plain(ts, 0, attempt_for_plain, text, id.pub_key(), &mut plain)
                .map_err(|_| SendError::Failed)?
        } else {
            // `sendCommandData`: neither extended-attempt tail nor ack.
            if text.len() > crate::node::TEXT_MAX {
                return Err(SendError::Failed);
            }
            let mut w = mc_core::Writer::new(&mut plain);
            w.put_u32_le(ts).map_err(|_| SendError::Failed)?;
            w.put_u8((attempt & 3) | (txt_type << 2))
                .map_err(|_| SendError::Failed)?;
            w.put(text).map_err(|_| SendError::Failed)?;
            (w.position(), 0)
        };
        let pkt = payload::datagram(
            PayloadType::TXT_MSG,
            dest,
            id.hash(),
            &secret,
            plain.get(..n).unwrap_or(&[]),
        )
        .map_err(|_| SendError::Failed)?;
        let (flood, timeout_ms) = self
            .send_to_contact(now, slot, pkt, 0)
            .ok_or(SendError::Failed)?;
        if txt_type == 0 {
            if let Some(e) = self.acks.get_mut(self.next_ack) {
                *e = Some(ExpectedAck {
                    ack: crc,
                    sent: now,
                    key,
                });
            }
            self.next_ack = (self.next_ack + 1) % crate::node::ACK_TABLE;
        }
        Ok(Sent {
            flood,
            ack: crc,
            timeout_ms,
        })
    }

    /// The answer to a console command a contact sent (`BaseChatMesh.cpp:263-292`): CLI data,
    /// stamped with a unique time that differs from the command's, 600 ms later
    /// (`CLI_REPLY_DELAY_MILLIS`), direct if the path is known, flooded within the scope if not.
    pub fn send_cli_reply(
        &mut self,
        now: Instant,
        slot: Slot,
        sender_ts: u32,
        text: &[u8],
    ) -> Result<(), SendError> {
        let mut ts = self.rtc.unique(now);
        if ts == sender_ts {
            ts = ts.wrapping_add(1);
        }
        let id = self.id.clone();
        let c = self.contacts.get_mut(slot).ok_or(SendError::NotFound)?;
        let secret = c.secret(&id).ok_or(SendError::Failed)?;
        let dest = c.hash();
        let mut plain = [0u8; 5 + crate::cli::REPLY_MAX];
        let mut w = mc_core::Writer::new(&mut plain);
        w.put_u32_le(ts).map_err(|_| SendError::Failed)?;
        w.put_u8(1 << 2).map_err(|_| SendError::Failed)?; // TXT_TYPE_CLI_DATA
        w.put(text).map_err(|_| SendError::Failed)?;
        let n = w.position();
        let pkt = payload::datagram(
            PayloadType::TXT_MSG,
            dest,
            id.hash(),
            &secret,
            plain.get(..n).unwrap_or(&[]),
        )
        .map_err(|_| SendError::Failed)?;
        self.send_to_contact(now, slot, pkt, CLI_REPLY_DELAY_MS)
            .ok_or(SendError::Failed)?;
        Ok(())
    }

    /// `SEND_CHANNEL_TXT_MSG`: `"<name>: <text>"` flooded within the scope.
    pub fn send_channel_text(
        &mut self,
        now: Instant,
        idx: u8,
        ts: u32,
        text: &[u8],
    ) -> Result<(), SendError> {
        let ch = self
            .channels
            .0
            .get(usize::from(idx))
            .ok_or(SendError::NotFound)?
            .clone();
        let mut plain = [0u8; 184];
        let name = self.prefs.name;
        let n = payload::group_txt_plain(ts, name_str(&name), text, &mut plain)
            .map_err(|_| SendError::Failed)?;
        let pkt = payload::group_datagram(
            PayloadType::GRP_TXT,
            &ch.secret,
            plain.get(..n).unwrap_or(&[]),
        )
        .map_err(|_| SendError::Failed)?;
        self.send_flood_scoped(pkt, 0, now);
        Ok(())
    }

    /// `SEND_CHANNEL_DATA`: flood within the scope (`path` = `None`) or given path
    /// (0 = zero hop).
    pub fn send_channel_data(
        &mut self,
        now: Instant,
        idx: u8,
        path: Option<(u8, &[u8])>,
        data_type: u16,
        data: &[u8],
    ) -> Result<(), SendError> {
        let ch = self
            .channels
            .0
            .get(usize::from(idx))
            .ok_or(SendError::NotFound)?
            .clone();
        let mut plain = [0u8; 184];
        let n = payload::group_data_plain(data_type, data, &mut plain)
            .map_err(|_| SendError::Failed)?;
        let pkt = payload::group_datagram(
            PayloadType::GRP_DATA,
            &ch.secret,
            plain.get(..n).unwrap_or(&[]),
        )
        .map_err(|_| SendError::Failed)?;
        match path {
            None => self.send_flood_scoped(pkt, 0, now),
            Some((len, bytes)) => self.send_direct(pkt, len, bytes, 0, now),
        }
        Ok(())
    }

    /// Our advert (`createSelfAdvert`): chat type, name, position if the policy allows it.
    pub fn self_advert(&self, now: Instant) -> Result<PacketBuf, SendError> {
        let name = self.prefs.name;
        let ad = AppData {
            kind: mc_air::advert::TYPE_CHAT,
            location: (self.prefs.advert_loc_policy != 0).then_some(self.position()),
            feat1: None,
            feat2: None,
            name: Some(name_str(&name)),
        };
        let mut buf = [0u8; 32];
        let n = ad.encode(&mut buf).map_err(|_| SendError::Failed)?;
        payload::advert(&self.id, self.rtc.now(now), buf.get(..n).unwrap_or(&[]))
            .map_err(|_| SendError::Failed)
    }

    /// `SEND_SELF_ADVERT`: zero hop, or flood within the default scope only.
    pub fn send_self_advert(&mut self, now: Instant, flood: bool) -> Result<(), SendError> {
        let pkt = self.self_advert(now)?;
        if flood {
            let key = self.prefs.default_scope_key;
            let transport = (key != [0; 16]).then(|| {
                [
                    mc_air::crypto::transport_code(
                        &key,
                        PayloadType::ADVERT.value(),
                        pkt.payload(),
                    ),
                    0,
                ]
            });
            self.send_flood(pkt, transport, 0, now);
        } else {
            self.send_zero_hop(pkt, None, 0, now);
        }
        Ok(())
    }

    /// `RESET_PATH`: the next send to the contact goes out as flood again.
    pub fn reset_path(&mut self, key: &[u8; PUB_KEY_LEN]) -> bool {
        let Some(s) = self.contacts.find(key) else {
            return false;
        };
        if let Some(c) = self.contacts.get_mut(s) {
            c.out_path_len = PATH_UNKNOWN;
        }
        true
    }

    /// `SEND_RAW_PACKET`: frame transmitted as is, with the given priority.
    pub fn send_raw_packet(
        &mut self,
        now: Instant,
        priority: u8,
        raw: &[u8],
    ) -> Result<(), SendError> {
        let p = mc_air::Packet::decode(raw).map_err(|_| SendError::Unsupported)?;
        let pkt = PacketBuf::from_packet(&p).map_err(|_| SendError::Unsupported)?;
        if self.disp.queue(pkt, priority, now) {
            Ok(())
        } else {
            Err(SendError::Failed)
        }
    }

    /// Validates a `path_len` coming from the app.
    pub fn valid_path_len(b: u8) -> bool {
        PathLen::from_byte(b).is_ok()
    }
}
