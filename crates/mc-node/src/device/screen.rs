//! What the device's own screen sends: messages with automatic retries (deliberate deviation
//! 6), and requests (ping, status, trace) whose answers go to the screen, not to the app.
//!
//! The app's single pending slots (`pending_*`) are left alone: a request from the screen and
//! one from the app can be in flight together.

use mc_core::{Duration, Instant};
use rand_core::Rng;

use super::{Device, Storage, UiEvent};
use crate::history::{HISTORY_TEXT, Msg, Peer, Status};
use crate::node::SendError;

/// Messages from the screen awaiting their acknowledgement.
pub const OUTBOX: usize = 8;
/// Requests from the screen in flight, and answers kept for it.
pub const REQUESTS: usize = 4;
/// Tries by the known route; then `RESET_PATH` and one flood.
const DIRECT_TRIES: u8 = 3;
const REQ_TYPE_GET_STATUS: u8 = 0x01;
const REQ_TYPE_GET_TELEMETRY_DATA: u8 = 0x03;

#[derive(Debug, Clone, Copy)]
pub(super) struct Outgoing {
    seq: u32,
    key: [u8; 32],
    ts: u32,
    attempt: u8,
    /// Expected acks of every attempt so far: a late one still counts.
    acks: [u32; 4],
    deadline: Instant,
    first_sent: Instant,
}

/// What a request from the screen asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqKind {
    /// Telemetry request, used as a ping.
    Telemetry,
    Status,
    Trace,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Pending {
    tag: u32,
    key4: u32,
    kind: ReqKind,
    sent: Instant,
    deadline: Instant,
}

/// The answer to a request from the screen, or its timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub tag: u32,
    pub kind: ReqKind,
    /// False: no answer before the timeout.
    pub ok: bool,
    pub rtt_ms: u32,
    /// SNR ×4 of the answer as we heard it (0 on a timeout).
    pub snr4: i8,
    /// Telemetry: CayenneLPP. Status: the status block. Trace: hashes, SNRs ×4, final SNR ×4.
    pub data: heapless::Vec<u8, 136>,
}

fn prefix6(k: &[u8; 32]) -> [u8; 6] {
    let mut p = [0u8; 6];
    p.copy_from_slice(&k[..6]);
    p
}

fn ms(now: Instant, since: Instant) -> u32 {
    u32::try_from(now.saturating_duration_since(since).as_millis()).unwrap_or(u32::MAX)
}

impl Device {
    /// The screen sends `text` to a contact; returns the history entry. Retried until
    /// acknowledged: three tries, then `RESET_PATH` and a flood, then failed.
    pub fn send_text(
        &mut self,
        now: Instant,
        key: &[u8; 32],
        text: &[u8],
        st: &mut impl Storage,
    ) -> Result<u32, SendError> {
        let text = text.get(..text.len().min(HISTORY_TEXT)).unwrap_or(&[]);
        let ts = self.node.rtc.unique(now);
        let sent = self.node.send_text(now, &prefix6(key), 0, 0, ts, text)?;
        let (seq, slot) = self.history.push(Msg {
            seq: 0,
            peer: Peer::Contact(prefix6(key)),
            outgoing: true,
            ts,
            at: self.node.time(now),
            status: Status::Sending { attempt: 0 },
            path_len: if sent.flood { 0 } else { 0xFF },
            snr4: 0,
            text: heapless::Vec::from_slice(text).unwrap_or_default(),
        });
        self.save_history_slot(slot, st);
        if self.outbox.is_full() {
            let old = self.outbox.remove(0);
            self.finish(old.seq, Status::Failed, st);
        }
        let _ = self.outbox.push(Outgoing {
            seq,
            key: *key,
            ts,
            attempt: 0,
            acks: [sent.ack, 0, 0, 0],
            deadline: now + Duration::from_millis(u64::from(sent.timeout_ms)),
            first_sent: now,
        });
        self.notify(UiEvent::Message {
            seq,
            peer: Peer::Contact(prefix6(key)),
            outgoing: true,
        });
        Ok(seq)
    }

    /// The screen sends `text` to a channel; nothing acknowledges it.
    pub fn send_channel_text(
        &mut self,
        now: Instant,
        idx: u8,
        text: &[u8],
        st: &mut impl Storage,
    ) -> Result<u32, SendError> {
        let text = text.get(..text.len().min(HISTORY_TEXT)).unwrap_or(&[]);
        let ts = self.node.rtc.unique(now);
        self.node.send_channel_text(now, idx, ts, text)?;
        let (seq, slot) = self.history.push(Msg {
            seq: 0,
            peer: Peer::Channel(idx),
            outgoing: true,
            ts,
            at: self.node.time(now),
            status: Status::Sent,
            path_len: 0,
            snr4: 0,
            text: heapless::Vec::from_slice(text).unwrap_or_default(),
        });
        self.save_history_slot(slot, st);
        self.notify(UiEvent::Message {
            seq,
            peer: Peer::Channel(idx),
            outgoing: true,
        });
        Ok(seq)
    }

    fn finish(&mut self, seq: u32, status: Status, st: &mut impl Storage) {
        if let Some(slot) = self.history.set_status(seq, status) {
            self.save_history_slot(slot, st);
        }
        self.notify(UiEvent::Status { seq });
    }

    /// An acknowledgement came in: true if it was for a message from the screen (the app
    /// must not hear about it).
    pub(super) fn confirm_outgoing(
        &mut self,
        now: Instant,
        ack: u32,
        st: &mut impl Storage,
    ) -> bool {
        let Some(i) = self
            .outbox
            .iter()
            .position(|o| o.acks.iter().any(|&a| a != 0 && a == ack))
        else {
            return false;
        };
        let o = self.outbox.remove(i);
        let rtt_ms = ms(now, o.first_sent);
        self.finish(o.seq, Status::Delivered { rtt_ms }, st);
        true
    }

    /// Retries, timeouts: called from `tick`.
    pub(super) fn screen_tick(&mut self, now: Instant, st: &mut impl Storage) {
        let mut i = 0;
        while i < self.outbox.len() {
            let Some(o) = self.outbox.get(i).copied() else {
                break;
            };
            if now < o.deadline {
                i += 1;
                continue;
            }
            let next = o.attempt + 1;
            let text = self.history.get(o.seq).map(|m| m.text.clone());
            let resent = match text {
                Some(text) if next <= DIRECT_TRIES => {
                    if next == DIRECT_TRIES {
                        self.node.reset_path(&o.key);
                    }
                    self.node
                        .send_text(now, &prefix6(&o.key), 0, next, o.ts, &text)
                        .ok()
                }
                _ => None,
            };
            match resent {
                Some(sent) => {
                    if let Some(e) = self.outbox.get_mut(i) {
                        e.attempt = next;
                        if let Some(a) = e.acks.get_mut(usize::from(next)) {
                            *a = sent.ack;
                        }
                        e.deadline = now + Duration::from_millis(u64::from(sent.timeout_ms));
                    }
                    self.finish(o.seq, Status::Sending { attempt: next }, st);
                    i += 1;
                }
                None => {
                    self.outbox.remove(i);
                    self.finish(o.seq, Status::Failed, st);
                }
            }
        }
        let mut j = 0;
        while j < self.ui_reqs.len() {
            match self.ui_reqs.get(j).copied() {
                Some(p) if now >= p.deadline => {
                    self.ui_reqs.remove(j);
                    self.answer(p, now, false, &[]);
                }
                _ => j += 1,
            }
        }
    }

    pub(super) fn screen_wakeup(&self) -> Option<Instant> {
        let a = self.outbox.iter().map(|o| o.deadline).min();
        let b = self.ui_reqs.iter().map(|p| p.deadline).min();
        match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The screen asks a contact for its telemetry (a ping) or status; returns the tag to
    /// find the answer with [`Device::request_reply`].
    pub fn request(
        &mut self,
        now: Instant,
        key: &[u8; 32],
        kind: ReqKind,
        rng: &mut impl Rng,
    ) -> Result<u32, SendError> {
        let slot = self.node.contacts.find(key).ok_or(SendError::NotFound)?;
        let req = match kind {
            ReqKind::Telemetry => REQ_TYPE_GET_TELEMETRY_DATA,
            ReqKind::Status => REQ_TYPE_GET_STATUS,
            ReqKind::Trace => return Err(SendError::Unsupported),
        };
        let sent = self.node.send_typed_request(now, slot, req, rng)?;
        self.track(Pending {
            tag: sent.ack,
            key4: u32::from_le_bytes(super::fixed(key)),
            kind,
            sent: now,
            deadline: now + Duration::from_millis(u64::from(sent.timeout_ms)),
        });
        Ok(sent.ack)
    }

    /// The screen traces a path of repeater hashes (`flags & 3`: hash size code).
    pub fn request_trace(
        &mut self,
        now: Instant,
        flags: u8,
        path: &[u8],
        rng: &mut impl Rng,
    ) -> Result<u32, SendError> {
        let tag = rng.next_u32();
        let timeout_ms = self.node.send_trace(now, tag, 0, flags, path)?;
        self.track(Pending {
            tag,
            key4: 0,
            kind: ReqKind::Trace,
            sent: now,
            deadline: now + Duration::from_millis(u64::from(timeout_ms)),
        });
        Ok(tag)
    }

    fn track(&mut self, p: Pending) {
        if self.ui_reqs.is_full() {
            let _ = self.ui_reqs.remove(0);
        }
        let _ = self.ui_reqs.push(p);
    }

    fn answer(&mut self, p: Pending, now: Instant, ok: bool, data: &[u8]) {
        let mut d = heapless::Vec::new();
        let _ = d.extend_from_slice(data.get(..data.len().min(136)).unwrap_or(&[]));
        if self.replies.is_full() {
            let _ = self.replies.pop_front();
        }
        // The answer is the last packet logged before its event.
        let snr4 = if ok {
            self.packets.recent().next().map(|p| p.snr4).unwrap_or(0)
        } else {
            0
        };
        let _ = self.replies.push_back(Reply {
            tag: p.tag,
            kind: p.kind,
            ok,
            rtt_ms: ms(now, p.sent),
            snr4,
            data: d,
        });
        self.notify(UiEvent::Reply { tag: p.tag });
    }

    /// The answer to a request from the screen, once it came (or timed out).
    pub fn request_reply(&self, tag: u32) -> Option<&Reply> {
        self.replies.iter().rev().find(|r| r.tag == tag)
    }

    /// A response from a contact: true if it answers a request from the screen.
    pub(super) fn screen_response(&mut self, now: Instant, key: &[u8; 32], data: &[u8]) -> bool {
        let tag = u32::from_le_bytes(super::fixed(data));
        let key4 = u32::from_le_bytes(super::fixed(key));
        let found = self.ui_reqs.iter().position(|p| match p.kind {
            ReqKind::Telemetry => data.len() > 4 && p.tag == tag,
            // Status answers carry the server's clock, not our tag (`MyMesh.cpp:694-776`).
            ReqKind::Status => data.len() > 4 && p.key4 == key4,
            ReqKind::Trace => false,
        });
        let Some(i) = found else {
            return false;
        };
        let p = self.ui_reqs.remove(i);
        self.answer(p, now, true, data.get(4..).unwrap_or(&[]));
        true
    }

    /// A trace came back: true if the screen sent it.
    pub(super) fn screen_trace(
        &mut self,
        now: Instant,
        tag: u32,
        hashes: &[u8],
        snrs: &[u8],
        final_snr4: i8,
    ) -> bool {
        let Some(i) = self
            .ui_reqs
            .iter()
            .position(|p| p.kind == ReqKind::Trace && p.tag == tag)
        else {
            return false;
        };
        let p = self.ui_reqs.remove(i);
        let mut d: heapless::Vec<u8, 136> = heapless::Vec::new();
        let _ = d.push(hashes.len() as u8);
        let _ = d.extend_from_slice(hashes);
        let _ = d.extend_from_slice(snrs);
        let _ = d.push(final_snr4.to_le_bytes()[0]);
        self.answer(p, now, true, &d);
        true
    }
}
