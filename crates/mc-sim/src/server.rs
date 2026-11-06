//! A simulated repeater's server side, for tests and the emulator: answers node discovery,
//! logins, status and neighbours requests the way MeshCore's `simple_repeater` (v1.18,
//! `examples/simple_repeater/MyMesh.cpp`) does, from the events of a simulated node with
//! repeating on. Not product code: oxymesh has no repeater role yet.

use mc_air::PayloadType;
use mc_core::Instant;
use mc_node::{Device, Event, Node};

/// `RESP_SERVER_LOGIN_OK`.
const LOGIN_OK: u8 = 0;
const REQ_STATUS: u8 = 0x01;
const REQ_NEIGHBOURS: u8 = 0x06;
const PERM_ADMIN: u8 = 3;
const PERM_GUEST: u8 = 1;
const FIRMWARE_VER_LEVEL: u8 = 1;
/// `SERVER_RESPONSE_DELAY`.
const DELAY_MS: u64 = 300;

#[derive(Debug, Clone)]
pub struct RepeaterServer {
    pub admin_password: String,
    pub guest_password: String,
    /// Logged-in clients: key, permissions, last request timestamp.
    acl: Vec<([u8; 32], u8, u32)>,
    /// Events already handled.
    seen: usize,
    counter: u32,
}

impl RepeaterServer {
    pub fn new(admin: &str, guest: &str) -> Self {
        Self {
            admin_password: String::from(admin),
            guest_password: String::from(guest),
            acl: Vec::new(),
            seen: 0,
            counter: 0,
        }
    }

    /// Answers what came in since the last call.
    pub fn serve(&mut self, dev: &mut Device, events: &[Event], now: Instant) {
        // The runner may clear the list between steps: everything left in it is new.
        if self.seen > events.len() {
            self.seen = 0;
        }
        let new = events.get(self.seen..).unwrap_or(&[]).to_vec();
        self.seen = events.len();
        for e in &new {
            match e {
                Event::AnonRequest { key, ts, data, .. } => {
                    self.login(&mut dev.node, now, key, *ts, data)
                }
                Event::Request { key, ts, data, .. } => self.request(dev, now, key, *ts, data),
                Event::ControlData { snr4, data, .. } => {
                    self.discover(&mut dev.node, now, *snr4, data)
                }
                _ => {}
            }
        }
    }

    fn reply(&self, node: &mut Node, now: Instant, key: &[u8; 32], plain: &[u8]) {
        let Some(secret) = node.id.shared_secret(key) else {
            return;
        };
        if let Ok(pkt) = mc_air::payload::datagram(
            PayloadType::RESPONSE,
            key[0],
            node.id.hash(),
            &secret,
            plain,
        ) {
            node.send_zero_hop(pkt, None, DELAY_MS, now);
        }
    }

    /// `handleLoginReq`: admin or guest password, or blank if already in the ACL.
    fn login(&mut self, node: &mut Node, now: Instant, key: &[u8; 32], ts: u32, data: &[u8]) {
        let pw_end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        let pw = String::from_utf8_lossy(&data[..pw_end]).into_owned();
        let known = self.acl.iter().position(|(k, _, _)| k == key);
        let perms = if pw.is_empty() && known.is_some() {
            known.map(|i| self.acl[i].1).unwrap_or(0)
        } else if pw == self.admin_password {
            PERM_ADMIN
        } else if pw == self.guest_password {
            PERM_GUEST
        } else {
            return;
        };
        match known {
            Some(i) => {
                if ts <= self.acl[i].2 {
                    return; // replay
                }
                self.acl[i] = (*key, perms, ts);
            }
            None => self.acl.push((*key, perms, ts)),
        }
        self.counter += 1;
        let now_s = node.time(now);
        let mut r = Vec::with_capacity(13);
        r.extend_from_slice(&now_s.to_le_bytes());
        r.push(LOGIN_OK);
        r.push(0);
        r.push(u8::from(perms == PERM_ADMIN));
        r.push(perms);
        r.extend_from_slice(&self.counter.to_le_bytes());
        r.push(FIRMWARE_VER_LEVEL);
        self.reply(node, now, key, &r);
    }

    /// `handleRequest`: status and neighbours, for clients in the ACL.
    fn request(&mut self, dev: &mut Device, now: Instant, key: &[u8; 32], ts: u32, data: &[u8]) {
        if !self.acl.iter().any(|(k, _, _)| k == key) {
            return;
        }
        let heard: Vec<([u8; 32], u32, i8)> = dev
            .heard()
            .sorted()
            .iter()
            .map(|n| (n.key, n.at, n.snr4))
            .collect();
        let node = &mut dev.node;
        let mut r: Vec<u8> = ts.to_le_bytes().to_vec();
        match data.first() {
            Some(&REQ_STATUS) => {
                let s = node.disp.stats;
                let le16 = |r: &mut Vec<u8>, v: u16| r.extend_from_slice(&v.to_le_bytes());
                let le32 = |r: &mut Vec<u8>, v: u32| r.extend_from_slice(&v.to_le_bytes());
                le16(&mut r, 4_050); // battery mV
                le16(&mut r, node.disp.len() as u16);
                le16(&mut r, (-112i16) as u16); // noise floor
                le16(&mut r, (-71i16) as u16); // last RSSI
                le32(&mut r, s.recv_flood + s.recv_direct);
                le32(&mut r, s.sent_flood + s.sent_direct);
                le32(&mut r, (s.tx_airtime_ms / 1000) as u32);
                le32(
                    &mut r,
                    node.time(now).saturating_sub(crate::EPOCH).max(3_600),
                );
                le32(&mut r, s.sent_flood);
                le32(&mut r, s.sent_direct);
                le32(&mut r, s.recv_flood);
                le32(&mut r, s.recv_direct);
                le16(&mut r, s.err_flags);
                le16(&mut r, 38); // last SNR × 4
                le16(&mut r, 0);
                le16(&mut r, 2);
                le32(&mut r, (s.rx_airtime_ms / 1000) as u32);
                le32(&mut r, s.recv_errors);
            }
            Some(&REQ_NEIGHBOURS) => {
                // [1] version 0, [2] count, [3..5] offset, [5] order, [6] key prefix length.
                let count = usize::from(data.get(2).copied().unwrap_or(0));
                let plen = usize::from(data.get(6).copied().unwrap_or(6)).min(32);
                // The simulated repeater's neighbours: the nodes it heard, strongest first;
                // at most 130 bytes of entries, like the C++.
                let mut sorted = heard.clone();
                sorted.sort_by_key(|n| std::cmp::Reverse(n.2));
                let now_s = node.time(now);
                let mut entries: Vec<u8> = Vec::new();
                let mut n = 0u16;
                for (k, at, snr) in sorted.iter().take(count) {
                    if entries.len() + plen + 5 > 130 {
                        break;
                    }
                    entries.extend_from_slice(&k[..plen]);
                    entries.extend_from_slice(&now_s.saturating_sub(*at).to_le_bytes());
                    entries.push(*snr as u8);
                    n += 1;
                }
                r.extend_from_slice(&(sorted.len() as u16).to_le_bytes());
                r.extend_from_slice(&n.to_le_bytes());
                r.extend_from_slice(&entries);
            }
            _ => return,
        }
        self.reply(node, now, key, &r);
    }

    /// `onControlDataRecv`: a repeater answers a discovery that asks for repeaters.
    fn discover(&mut self, node: &mut Node, now: Instant, snr4: i8, data: &[u8]) {
        let (Some(&head), Some(&filter), Some(tag)) = (data.first(), data.get(1), data.get(2..6))
        else {
            return;
        };
        if head & 0xF0 != 0x80 || filter & (1 << 2) == 0 {
            return;
        }
        let mut d = vec![0x90 | 2, snr4 as u8];
        d.extend_from_slice(tag);
        if head & 1 != 0 {
            d.extend_from_slice(&node.id.pub_key()[..8]);
        } else {
            d.extend_from_slice(node.id.pub_key());
        }
        let _ = node.send_control(now, &d);
    }
}
