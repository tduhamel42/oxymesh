//! Local message history for the screen (`docs/spec/ui.md` §9): the last 200 messages,
//! received and sent from the device, in RAM and saved to flash one slot at a time.
//!
//! Independent of the protocol's offline queue: the app still gets every message through
//! `SYNC_NEXT_MESSAGE`, and fetching it there marks it read here.

use mc_core::{Reader, Writer};

/// Messages kept.
pub const HISTORY: usize = 200;
/// Longest text kept (a DM carries at most 160 bytes; longer channel lines are cut).
pub const HISTORY_TEXT: usize = 160;
/// Size of a stored record.
pub const HISTORY_RECORD: usize = 192;
const RECORD_VERSION: u8 = 1;

/// Who the conversation is with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Peer {
    /// A contact, by the first 6 bytes of its key (what the app protocol uses).
    Contact([u8; 6]),
    Channel(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Received, not read yet.
    Unread,
    Read,
    /// Sent from the device, waiting for the acknowledgement; `attempt` 0 to 3 (3: flood).
    Sending {
        attempt: u8,
    },
    Delivered {
        rtt_ms: u32,
    },
    /// Sent to a channel: nothing acknowledges it.
    Sent,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    /// Increases by one per stored message; never reused.
    pub seq: u32,
    pub peer: Peer,
    pub outgoing: bool,
    /// The sender's timestamp (received) or ours (sent).
    pub ts: u32,
    /// Our clock when stored.
    pub at: u32,
    pub status: Status,
    /// As the app sees it: hops if received by flood, 0xFF if direct.
    pub path_len: u8,
    pub snr4: i8,
    pub text: heapless::Vec<u8, HISTORY_TEXT>,
}

impl Msg {
    pub fn is_unread(&self) -> bool {
        self.status == Status::Unread
    }
}

#[derive(Debug)]
pub struct History {
    slots: [Option<Msg>; HISTORY],
    next_seq: u32,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub const fn new() -> Self {
        Self {
            slots: [const { None }; HISTORY],
            next_seq: 1,
        }
    }

    fn slot_of(seq: u32) -> usize {
        seq as usize % HISTORY
    }

    /// Stores a message (its `seq` is assigned here); returns the slot to save.
    pub fn push(&mut self, mut m: Msg) -> (u32, usize) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);
        m.seq = seq;
        let slot = Self::slot_of(seq);
        if let Some(s) = self.slots.get_mut(slot) {
            *s = Some(m);
        }
        (seq, slot)
    }

    /// A record read back from flash at boot.
    pub fn restore(&mut self, m: Msg) {
        let slot = Self::slot_of(m.seq);
        let seq = m.seq;
        if let Some(s) = self.slots.get_mut(slot)
            // Equal: a later version of the same message (status change) wins.
            && s.as_ref().is_none_or(|old| old.seq <= seq)
        {
            *s = Some(m);
            self.next_seq = self.next_seq.max(seq.wrapping_add(1));
        }
    }

    pub fn get(&self, seq: u32) -> Option<&Msg> {
        self.slots
            .get(Self::slot_of(seq))?
            .as_ref()
            .filter(|m| m.seq == seq)
    }

    /// Changes a message's status; returns its slot (to save) if it changed.
    pub fn set_status(&mut self, seq: u32, status: Status) -> Option<usize> {
        let slot = Self::slot_of(seq);
        let m = self
            .slots
            .get_mut(slot)?
            .as_mut()
            .filter(|m| m.seq == seq)?;
        if m.status == status {
            return None;
        }
        m.status = status;
        Some(slot)
    }

    /// All messages, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Msg> {
        let start = Self::slot_of(self.next_seq);
        (0..HISTORY).filter_map(move |i| self.slots.get((start + i) % HISTORY)?.as_ref())
    }

    /// The conversation with `peer`, oldest first.
    pub fn with(&self, peer: Peer) -> impl Iterator<Item = &Msg> {
        self.iter().filter(move |m| m.peer == peer)
    }

    pub fn unread(&self) -> usize {
        self.iter().filter(|m| m.is_unread()).count()
    }

    pub fn unread_with(&self, peer: Peer) -> usize {
        self.with(peer).filter(|m| m.is_unread()).count()
    }

    /// Marks the conversation read; returns the slots to save.
    pub fn mark_read(&mut self, peer: Peer) -> heapless::Vec<usize, HISTORY> {
        let mut changed = heapless::Vec::new();
        for (i, s) in self.slots.iter_mut().enumerate() {
            if let Some(m) = s
                && m.peer == peer
                && m.status == Status::Unread
            {
                m.status = Status::Read;
                let _ = changed.push(i);
            }
        }
        changed
    }

    /// The record of a slot, to save.
    pub fn record(&self, slot: usize) -> Option<[u8; HISTORY_RECORD]> {
        self.slots.get(slot)?.as_ref().map(encode)
    }

    /// Conversations, the most recent first: (peer, last message).
    pub fn conversations(&self) -> heapless::Vec<(Peer, &Msg), 64> {
        let mut out: heapless::Vec<(Peer, &Msg), 64> = heapless::Vec::new();
        for m in self.iter() {
            if let Some(e) = out.iter_mut().find(|(p, _)| *p == m.peer) {
                e.1 = m;
            } else if out.push((m.peer, m)).is_err() {
                break;
            }
        }
        out.sort_unstable_by_key(|(_, m)| core::cmp::Reverse(m.seq));
        out
    }
}

pub fn encode(m: &Msg) -> [u8; HISTORY_RECORD] {
    let mut out = [0u8; HISTORY_RECORD];
    let mut w = Writer::new(&mut out);
    let _ = (|| {
        w.put_u8(RECORD_VERSION)?;
        w.put_u32_le(m.seq)?;
        match m.peer {
            Peer::Contact(p) => {
                w.put_u8(0)?;
                w.put(&p)?;
            }
            Peer::Channel(i) => {
                w.put_u8(1)?;
                w.put(&[i, 0, 0, 0, 0, 0])?;
            }
        }
        w.put_u8(u8::from(m.outgoing))?;
        w.put_u32_le(m.ts)?;
        w.put_u32_le(m.at)?;
        let (tag, v) = match m.status {
            Status::Unread => (0, 0),
            Status::Read => (1, 0),
            Status::Sending { attempt } => (2, u32::from(attempt)),
            Status::Delivered { rtt_ms } => (3, rtt_ms),
            Status::Sent => (4, 0),
            Status::Failed => (5, 0),
        };
        w.put_u8(tag)?;
        w.put_u32_le(v)?;
        w.put_u8(m.path_len)?;
        w.put(&m.snr4.to_le_bytes())?;
        w.put_u8(m.text.len() as u8)?;
        w.put(&m.text)
    })();
    out
}

pub fn decode(b: &[u8]) -> Option<Msg> {
    let mut r = Reader::new(b);
    if r.u8().ok()? != RECORD_VERSION {
        return None;
    }
    let seq = r.u32_le().ok()?;
    let kind = r.u8().ok()?;
    let p = *r.array::<6>().ok()?;
    let peer = match kind {
        0 => Peer::Contact(p),
        1 => Peer::Channel(p[0]),
        _ => return None,
    };
    let outgoing = r.u8().ok()? != 0;
    let ts = r.u32_le().ok()?;
    let at = r.u32_le().ok()?;
    let tag = r.u8().ok()?;
    let v = r.u32_le().ok()?;
    let status = match tag {
        0 => Status::Unread,
        1 => Status::Read,
        // Interrupted by a reboot: it will not be retried.
        2 => Status::Failed,
        3 => Status::Delivered { rtt_ms: v },
        4 => Status::Sent,
        5 => Status::Failed,
        _ => return None,
    };
    let path_len = r.u8().ok()?;
    let snr4 = i8::from_le_bytes([r.u8().ok()?]);
    let n = usize::from(r.u8().ok()?);
    let text = heapless::Vec::from_slice(r.take(n.min(HISTORY_TEXT)).ok()?).ok()?;
    if seq == 0 {
        return None;
    }
    Some(Msg {
        seq,
        peer,
        outgoing,
        ts,
        at,
        status,
        path_len,
        snr4,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(peer: Peer, text: &[u8]) -> Msg {
        Msg {
            seq: 0,
            peer,
            outgoing: false,
            ts: 100,
            at: 101,
            status: Status::Unread,
            path_len: 0xFF,
            snr4: -12,
            text: heapless::Vec::from_slice(text).unwrap(),
        }
    }

    #[test]
    fn ring_keeps_the_last_200_and_counts_unread() {
        let mut h = History::new();
        let alice = Peer::Contact([1; 6]);
        for i in 0..250u32 {
            h.push(msg(
                if i % 2 == 0 { alice } else { Peer::Channel(0) },
                b"hi",
            ));
        }
        assert_eq!(h.iter().count(), HISTORY);
        assert_eq!(h.iter().next().unwrap().seq, 51);
        assert_eq!(h.iter().last().unwrap().seq, 250);
        assert!(h.get(50).is_none());
        assert_eq!(h.unread(), 200);
        assert_eq!(h.mark_read(alice).len(), 100);
        assert_eq!(h.unread_with(alice), 0);
        assert_eq!(h.unread(), 100);
        let conv = h.conversations();
        assert_eq!(conv.len(), 2);
        assert_eq!(conv[0].0, Peer::Channel(0), "most recent first");
    }

    #[test]
    fn records_roundtrip_and_restore_continues_the_sequence() {
        let mut h = History::new();
        let (seq, slot) = h.push(msg(Peer::Contact([9; 6]), "café — ok".as_bytes()));
        h.set_status(seq, Status::Delivered { rtt_ms: 1234 });
        let rec = h.record(slot).unwrap();
        let back = decode(&rec).unwrap();
        assert_eq!(&back, h.get(seq).unwrap());
        let mut h2 = History::new();
        h2.restore(back);
        let (next, _) = h2.push(msg(Peer::Channel(3), b"x"));
        assert_eq!(next, seq + 1);
        // A message still sending when the device stopped comes back as failed.
        let mut m = msg(Peer::Contact([2; 6]), b"y");
        m.seq = 7;
        m.outgoing = true;
        m.status = Status::Sending { attempt: 1 };
        assert_eq!(decode(&encode(&m)).unwrap().status, Status::Failed);
    }
}
