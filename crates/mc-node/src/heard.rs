//! What the radio heard, for the Tools screens (`docs/spec/ui.md` §6): the nodes whose adverts
//! came in, and a log of the last packets.

use mc_air::Packet;

/// Nodes remembered.
pub const HEARD: usize = 32;
/// Packets remembered.
pub const PACKET_LOG: usize = 64;

/// A node heard through its advert, contact or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeardNode {
    pub key: [u8; 32],
    /// 0x00-terminated, padded.
    pub name: [u8; 32],
    /// 1 chat, 2 repeater, 3 room, 4 sensor.
    pub kind: u8,
    /// Hops the advert travelled (0: heard directly).
    pub hops: u8,
    pub snr4: i8,
    /// Our clock, Unix seconds.
    pub at: u32,
    /// Degrees × 1e6, if the advert carried a position.
    pub location: Option<(i32, i32)>,
    /// Adverts heard from it.
    pub count: u16,
}

#[derive(Debug)]
pub struct Heard {
    nodes: heapless::Vec<HeardNode, HEARD>,
}

impl Default for Heard {
    fn default() -> Self {
        Self::new()
    }
}

impl Heard {
    pub const fn new() -> Self {
        Self {
            nodes: heapless::Vec::new(),
        }
    }

    /// Records an advert; when full, the node heard longest ago makes room.
    pub fn note(&mut self, mut n: HeardNode) {
        if let Some(old) = self.nodes.iter_mut().find(|x| x.key == n.key) {
            n.count = old.count.saturating_add(1);
            if n.location.is_none() {
                n.location = old.location;
            }
            *old = n;
            return;
        }
        n.count = 1;
        if self.nodes.is_full()
            && let Some(i) = self
                .nodes
                .iter()
                .enumerate()
                .min_by_key(|(_, x)| x.at)
                .map(|(i, _)| i)
        {
            self.nodes.swap_remove(i);
        }
        let _ = self.nodes.push(n);
    }

    /// Repeaters first, then the most recently heard.
    pub fn sorted(&self) -> heapless::Vec<&HeardNode, HEARD> {
        let mut v: heapless::Vec<&HeardNode, HEARD> = self.nodes.iter().collect();
        v.sort_unstable_by_key(|n| (n.kind != 2, core::cmp::Reverse(n.at)));
        v
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The node heard last.
    pub fn last(&self) -> Option<&HeardNode> {
        self.nodes.iter().max_by_key(|n| n.at)
    }
}

/// One received packet, summarized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggedPacket {
    /// Our clock, Unix seconds.
    pub at: u32,
    /// Payload type (4 bits) and route type (2 bits), as on air.
    pub ptype: u8,
    pub route: u8,
    pub hops: u8,
    pub len: u8,
    pub snr4: i8,
    pub rssi: i8,
    /// First two payload bytes: destination and source hashes for addressed packets.
    pub head: [u8; 2],
    /// False if the frame did not decode as MeshCore.
    pub valid: bool,
}

impl LoggedPacket {
    pub fn of(raw: &[u8], at: u32, snr4: i8, rssi: i8) -> Self {
        let len = raw.len().min(255) as u8;
        match Packet::decode(raw) {
            Ok(p) => Self {
                at,
                ptype: p.header.ptype.value(),
                route: p.header.route as u8,
                hops: p.path_len.hops(),
                len,
                snr4,
                rssi,
                head: [
                    p.payload.first().copied().unwrap_or(0),
                    p.payload.get(1).copied().unwrap_or(0),
                ],
                valid: true,
            },
            Err(_) => Self {
                at,
                ptype: 0,
                route: 0,
                hops: 0,
                len,
                snr4,
                rssi,
                head: [0; 2],
                valid: false,
            },
        }
    }
}

#[derive(Debug)]
pub struct PacketLog {
    ring: heapless::Deque<LoggedPacket, PACKET_LOG>,
    /// Packets seen since boot.
    pub total: u32,
}

impl Default for PacketLog {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketLog {
    pub const fn new() -> Self {
        Self {
            ring: heapless::Deque::new(),
            total: 0,
        }
    }

    pub fn push(&mut self, p: LoggedPacket) {
        if self.ring.is_full() {
            let _ = self.ring.pop_front();
        }
        let _ = self.ring.push_back(p);
        self.total = self.total.wrapping_add(1);
    }

    /// The most recent first.
    pub fn recent(&self) -> impl Iterator<Item = &LoggedPacket> {
        self.ring.iter().rev()
    }
}

/// Minutes of radio history kept (the Signal and Airtime tools).
pub const MINUTES: usize = 60;

/// One minute of radio activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Minute {
    /// Packets received.
    pub rx: u16,
    /// Transmit airtime, ms.
    pub tx_ms: u32,
    /// Noise floor at the end of the minute, dBm.
    pub noise: i16,
}

/// The last hour, minute by minute.
#[derive(Debug)]
pub struct Minutes {
    ring: heapless::Deque<Minute, MINUTES>,
    start: Option<mc_core::Instant>,
    base_rx: u32,
    base_tx_ms: u64,
}

impl Default for Minutes {
    fn default() -> Self {
        Self::new()
    }
}

impl Minutes {
    pub const fn new() -> Self {
        Self {
            ring: heapless::Deque::new(),
            start: None,
            base_rx: 0,
            base_tx_ms: 0,
        }
    }

    /// Closes the minutes that ended before `now`, from the running totals.
    pub fn update(&mut self, now: mc_core::Instant, rx_total: u32, tx_ms_total: u64, noise: i16) {
        let minute = mc_core::Duration::from_secs(60);
        let Some(mut start) = self.start else {
            self.start = Some(now);
            self.base_rx = rx_total;
            self.base_tx_ms = tx_ms_total;
            return;
        };
        let mut first = true;
        while now.saturating_duration_since(start) >= minute {
            let m = if first {
                Minute {
                    rx: u16::try_from(rx_total.wrapping_sub(self.base_rx)).unwrap_or(u16::MAX),
                    tx_ms: u32::try_from(tx_ms_total.saturating_sub(self.base_tx_ms))
                        .unwrap_or(u32::MAX),
                    noise,
                }
            } else {
                // Whole minutes without a tick: nothing seen.
                Minute {
                    rx: 0,
                    tx_ms: 0,
                    noise,
                }
            };
            first = false;
            if self.ring.is_full() {
                let _ = self.ring.pop_front();
            }
            let _ = self.ring.push_back(m);
            start = start + minute;
        }
        if !first {
            self.base_rx = rx_total;
            self.base_tx_ms = tx_ms_total;
        }
        self.start = Some(start);
    }

    /// Oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Minute> {
        self.ring.iter()
    }

    /// Transmit airtime over the minutes kept, ms.
    pub fn tx_ms(&self) -> u64 {
        self.ring.iter().map(|m| u64::from(m.tx_ms)).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(k: u8, kind: u8, at: u32) -> HeardNode {
        HeardNode {
            key: [k; 32],
            name: [0; 32],
            kind,
            hops: 0,
            snr4: 0,
            at,
            location: None,
            count: 0,
        }
    }

    #[test]
    fn heard_updates_in_place_and_evicts_the_oldest() {
        let mut h = Heard::new();
        h.note(node(1, 1, 10));
        let mut again = node(1, 1, 20);
        again.location = None;
        h.note(again);
        assert_eq!(h.len(), 1);
        assert_eq!(h.sorted()[0].count, 2);
        for k in 2..=40u8 {
            h.note(node(k, if k == 30 { 2 } else { 1 }, 100 + u32::from(k)));
        }
        assert_eq!(h.len(), HEARD);
        assert!(
            h.sorted().iter().all(|n| n.key != [1; 32]),
            "oldest evicted"
        );
        assert_eq!(h.sorted()[0].kind, 2, "repeaters first");
        assert_eq!(h.last().unwrap().key, [40; 32]);
    }

    #[test]
    fn minutes_close_on_time() {
        use mc_core::{Duration, Instant};
        let mut m = Minutes::new();
        let t0 = Instant::ZERO + Duration::from_secs(5);
        m.update(t0, 10, 1000, -110);
        m.update(t0 + Duration::from_secs(30), 12, 1500, -110);
        assert_eq!(m.iter().count(), 0);
        m.update(t0 + Duration::from_secs(61), 15, 1800, -108);
        assert_eq!(
            m.iter().copied().collect::<heapless::Vec<_, 4>>(),
            [Minute {
                rx: 5,
                tx_ms: 800,
                noise: -108
            }]
        );
        // Three minutes later: one with activity, two empty.
        m.update(t0 + Duration::from_secs(61 + 180), 16, 1900, -105);
        assert_eq!(m.iter().count(), 4);
        assert_eq!(m.tx_ms(), 900);
    }

    #[test]
    fn packet_log_is_a_ring() {
        let mut l = PacketLog::new();
        for i in 0..70u32 {
            l.push(LoggedPacket::of(&[0xFF], i, 0, 0));
        }
        assert_eq!(l.total, 70);
        assert_eq!(l.recent().count(), PACKET_LOG);
        assert_eq!(l.recent().next().unwrap().at, 69);
        assert!(!l.recent().next().unwrap().valid);
    }
}
