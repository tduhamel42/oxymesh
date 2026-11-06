//! Companion layer (`examples/companion_radio/MyMesh.cpp`; `docs/spec/companion.md`):
//! app commands, offline message queue, notifications, contact iteration, server requests.
//! The node (`Node`) handles the network; `Device` talks to the app.

mod screen;
mod settings;

pub use screen::{OUTBOX, REQUESTS, Reply, ReqKind};
pub use settings::SetResult;

use mc_air::crypto::{LocalIdentity, PUB_KEY_LEN};
use mc_air::{PathLen, payload};
use mc_core::{Duration, Instant};
use mc_proto::codec::{DeviceReject, err};
use mc_proto::response::{Contact as RContact, DeviceInfo, LoginServer, SelfInfo, Stats};
use mc_proto::{Command, Response};
use rand_core::Rng;

use crate::contacts::{Contact, MAX_CHANNELS, MAX_CONTACTS, PATH_UNKNOWN, Slot, name_str};
use crate::heard::{Heard, HeardNode, LoggedPacket, Minutes, PacketLog};
use crate::history::{HISTORY_RECORD, HISTORY_TEXT, History, Msg, Peer, Status};
use crate::node::{Event, Node, SendError, Sent};
use crate::persist::{CHANNEL_RECORD, CONTACT_RECORD, PREFS_RECORD, encode_contact, encode_prefs};

/// Frame to the app (`MAX_FRAME_SIZE`).
pub const FRAME_MAX: usize = 176;
pub type Frame = heapless::Vec<u8, FRAME_MAX>;
/// Offline message queue of the M5 build (`OFFLINE_QUEUE_SIZE`).
pub const OFFLINE_QUEUE: usize = 256;
/// `FIRMWARE_VER_CODE` of MeshCore v1.18.
pub const FIRMWARE_VER_CODE: u8 = 14;
/// Maximum size of a message to sign (`SIGN_START`).
pub const SIGN_MAX: usize = 8192;
/// Deferred contacts write (`MyMesh.cpp`, 5 s).
const CONTACTS_SAVE_DELAY: Duration = Duration::from_secs(5);
/// Server response delay (`SERVER_RESPONSE_DELAY`).
const SERVER_RESPONSE_DELAY: u64 = 300;
/// Frequencies where companion repeating is allowed (`MyMesh.cpp:1032-1040`), in kHz.
pub const REPEAT_FREQS: [u32; 3] = [433_000, 869_495, 918_000];
const REQ_TYPE_GET_TELEMETRY_DATA: u8 = 0x03;

/// Persistent storage provided by the runner.
pub trait Storage {
    fn save_identity(&mut self, prv: &[u8; 64], pub_key: &[u8; PUB_KEY_LEN]);
    fn save_prefs(&mut self, rec: &[u8; PREFS_RECORD]);
    /// The whole contacts table, in order.
    fn save_contacts(&mut self, records: &mut dyn Iterator<Item = [u8; CONTACT_RECORD]>);
    fn save_channel(&mut self, idx: u8, rec: &[u8; CHANNEL_RECORD]);
    /// A contact's stored advert (at most 255 bytes).
    fn put_blob(&mut self, key: &[u8; PUB_KEY_LEN], raw: &[u8]);
    fn get_blob(&mut self, key: &[u8; PUB_KEY_LEN], out: &mut [u8]) -> Option<usize>;
    fn remove_blob(&mut self, key: &[u8; PUB_KEY_LEN]);
    /// (used, total) in KB.
    fn usage_kb(&self) -> (u32, u32);
    /// Offline queue entry `seq`: the history link (4 bytes) then the frame (deliberate
    /// deviation 1: the queue lives in flash and survives a reboot).
    fn put_queued(&mut self, seq: u32, entry: &[u8]);
    fn get_queued(&mut self, seq: u32, out: &mut [u8]) -> Option<usize>;
    fn remove_queued(&mut self, seq: u32);
    /// Slot of the local message history (`docs/spec/ui.md` §9).
    fn save_history(&mut self, slot: u16, rec: &[u8; HISTORY_RECORD]);
}

/// Something the screen should look at (`Device::poll_ui`). The details are read from the
/// device: history, heard nodes, packet log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    /// A message was stored in the history (received, or sent from the device).
    Message {
        seq: u32,
        peer: Peer,
        outgoing: bool,
    },
    /// A stored message changed status (read, delivered, failed).
    Status { seq: u32 },
    /// An advert was heard.
    Heard,
    /// Contacts were added, changed or removed.
    Contacts,
    /// A request from the screen was answered or timed out (`Device::request_reply`).
    Reply { tag: u32 },
}

/// Offline queue index entry; the frame itself is in storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Queued {
    seq: u32,
    channel: bool,
    /// History message it carries (0: none).
    hist: u32,
}

/// Largest stored queue entry: history link and frame.
pub const QUEUED_MAX: usize = 4 + FRAME_MAX;

/// What the board reports in `DEVICE_INFO`.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub manufacturer: &'static str,
    pub version: &'static str,
    /// Format `"dd-Mmm-yyyy"`.
    pub build_date: &'static str,
    pub max_tx_power: i8,
    pub has_gps: bool,
}

/// Actions requested from the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Reboot,
    FactoryReset,
    /// Radio or power settings changed: reconfigure the SX1262.
    RadioChanged,
    /// PIN code changed (applied at reboot, like in C++).
    PinChanged,
}

#[derive(Debug, Clone)]
struct AdvertPath {
    prefix: [u8; 7],
    recv_ts: u32,
    path_len: u8,
    path: heapless::Vec<u8, 64>,
}

#[derive(Debug, Clone, Copy)]
struct Iter {
    since: u32,
    next: usize,
    most_recent: u32,
}

pub struct Device {
    pub node: Node,
    pub info: Info,
    pub app_target_ver: u8,
    /// A transport is connected (over USB: always).
    pub connected: bool,
    pub battery_mv: u16,
    pub noise_floor: i16,
    pub mcu_temp_deci: Option<i16>,
    out: heapless::Deque<Frame, 8>,
    queue: heapless::Deque<Queued, OFFLINE_QUEUE>,
    queue_seq: u32,
    history: History,
    heard: Heard,
    packets: PacketLog,
    minutes: Minutes,
    outbox: heapless::Vec<screen::Outgoing, OUTBOX>,
    ui_reqs: heapless::Vec<screen::Pending, REQUESTS>,
    replies: heapless::Deque<Reply, REQUESTS>,
    iter: Option<Iter>,
    pending_login: u32,
    pending_status: u32,
    pending_telemetry: u32,
    pending_req: u32,
    sign: Option<heapless::Vec<u8, SIGN_MAX>>,
    advert_paths: heapless::Deque<AdvertPath, 16>,
    contacts_dirty: Option<Instant>,
    actions: heapless::Deque<Action, 4>,
    ui: heapless::Deque<UiEvent, 32>,
    /// Copies of every node event, for the simulator's tests.
    #[cfg(feature = "tap")]
    tap: heapless::Deque<Event, 64>,
    /// Custom variables (`GET_CUSTOM_VARS`): the `gps` setting.
    pub gps_interval: u32,
}

impl core::fmt::Debug for Device {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Device")
            .field("node", &self.node)
            .field("queue", &self.queue.len())
            .finish_non_exhaustive()
    }
}

fn frame_of(r: &Response<'_>) -> Option<Frame> {
    let mut buf = [0u8; FRAME_MAX];
    let n = r.encode(&mut buf).ok()?;
    Frame::from_slice(buf.get(..n)?).ok()
}

fn fixed<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    for (o, b) in out.iter_mut().zip(s.iter()) {
        *o = *b;
    }
    out
}

fn le4(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

fn prefix6(k: &[u8; PUB_KEY_LEN]) -> [u8; 6] {
    fixed(k)
}

fn rcontact(c: &Contact) -> RContact<'static> {
    RContact {
        key: c.pub_key,
        kind: c.kind,
        flags: c.flags,
        path_len: c.out_path_len,
        path: c.out_path,
        name: c.name,
        last_advert: c.last_advert,
        lat: c.lat,
        lon: c.lon,
        lastmod: c.lastmod,
        ext: &[],
    }
}

/// Error code of a failed send.
fn err_of(e: SendError) -> u8 {
    match e {
        SendError::NotFound => err::NOT_FOUND,
        SendError::Failed => err::TABLE_FULL,
        SendError::Unsupported => err::UNSUPPORTED,
    }
}

impl Device {
    pub fn new(node: Node, info: Info) -> Self {
        let mut d = Self::empty(info);
        d.node = node;
        d
    }

    /// Empty device, constructible at compile time (≈ 150 KB: it lives in a static on the
    /// device); the node is then initialised by `node.reset`.
    pub const fn empty(info: Info) -> Self {
        Self {
            node: Node::empty(),
            info,
            app_target_ver: 0,
            connected: false,
            battery_mv: 0,
            noise_floor: -120,
            mcu_temp_deci: None,
            out: heapless::Deque::new(),
            queue: heapless::Deque::new(),
            queue_seq: 1,
            history: History::new(),
            heard: Heard::new(),
            packets: PacketLog::new(),
            minutes: Minutes::new(),
            outbox: heapless::Vec::new(),
            ui_reqs: heapless::Vec::new(),
            replies: heapless::Deque::new(),
            iter: None,
            pending_login: 0,
            pending_status: 0,
            pending_telemetry: 0,
            pending_req: 0,
            sign: None,
            advert_paths: heapless::Deque::new(),
            contacts_dirty: None,
            actions: heapless::Deque::new(),
            ui: heapless::Deque::new(),
            #[cfg(feature = "tap")]
            tap: heapless::Deque::new(),
            gps_interval: 0,
        }
    }

    // ------------------------------------------------------------------ runner interface

    pub fn on_rx(
        &mut self,
        now: Instant,
        raw: &[u8],
        snr4: i8,
        rssi: i16,
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) {
        self.node.on_rx(now, raw, snr4, rssi, rng);
        self.pump(now, rng, st);
    }

    /// Call regularly: node events, deferred writes.
    pub fn tick(&mut self, now: Instant, rng: &mut impl Rng, st: &mut impl Storage) {
        self.pump(now, rng, st);
        self.screen_tick(now, st);
        self.pump(now, rng, st);
        let s = self.node.disp.stats;
        self.minutes
            .update(now, self.packets.total, s.tx_airtime_ms, self.noise_floor);
        if let Some(at) = self.contacts_dirty
            && now >= at
        {
            self.flush_contacts(st);
        }
    }

    /// Writes pending contacts right away (before a reboot).
    pub fn flush_contacts(&mut self, st: &mut impl Storage) {
        self.contacts_dirty = None;
        let mut it = self.node.contacts.list.iter().map(encode_contact);
        st.save_contacts(&mut it);
    }

    pub fn poll_tx(
        &mut self,
        now: Instant,
        busy: bool,
        rng: &mut impl Rng,
    ) -> Option<crate::dispatch::TxFrame> {
        self.node.poll_tx(now, busy, rng)
    }

    pub fn tx_done(&mut self, now: Instant) {
        self.node.tx_done(now);
    }

    pub fn next_wakeup(&self) -> Option<Instant> {
        [
            self.node.next_wakeup(),
            self.contacts_dirty,
            self.screen_wakeup(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Next frame to the app. The contacts of a `GET_CONTACTS` come out one by one, when the
    /// queue is empty (`checkSerialInterface`).
    pub fn poll_app(&mut self) -> Option<Frame> {
        if let Some(f) = self.out.pop_front() {
            return Some(f);
        }
        let mut it = self.iter?;
        let list = &self.node.contacts.list;
        while let Some(c) = list.get(it.next) {
            it.next += 1;
            if c.lastmod > it.since {
                it.most_recent = it.most_recent.max(c.lastmod);
                self.iter = Some(it);
                return frame_of(&Response::Contact(rcontact(c)));
            }
        }
        self.iter = None;
        frame_of(&Response::EndOfContacts {
            lastmod: it.most_recent,
        })
    }

    pub fn poll_action(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    /// Notifications for the device's screen.
    pub fn poll_ui(&mut self) -> Option<UiEvent> {
        self.ui.pop_front()
    }

    /// Every node event, copied (simulator and tests).
    #[cfg(feature = "tap")]
    pub fn poll_tap(&mut self) -> Option<Event> {
        self.tap.pop_front()
    }

    fn notify(&mut self, e: UiEvent) {
        if self.ui.is_full() {
            let _ = self.ui.pop_front();
        }
        let _ = self.ui.push_back(e);
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn heard(&self) -> &Heard {
        &self.heard
    }

    pub fn packets(&self) -> &PacketLog {
        &self.packets
    }

    /// The last hour of radio activity, minute by minute.
    pub fn minutes(&self) -> &Minutes {
        &self.minutes
    }

    /// Frames waiting to be transmitted.
    pub fn tx_queue(&self) -> usize {
        self.node.disp.len()
    }

    fn save_history_slot(&self, slot: usize, st: &mut impl Storage) {
        if let Some(rec) = self.history.record(slot) {
            st.save_history(slot as u16, &rec);
        }
    }

    /// The screen opened the conversation: its messages are read.
    pub fn mark_read(&mut self, peer: Peer, st: &mut impl Storage) {
        let slots = self.history.mark_read(peer);
        for slot in slots {
            self.save_history_slot(slot, st);
            if let Some(rec) = self.history.record(slot)
                && let Some(m) = crate::history::decode(&rec)
            {
                self.notify(UiEvent::Status { seq: m.seq });
            }
        }
    }

    /// Boot: a history record read back from storage.
    pub fn restore_history(&mut self, rec: &[u8]) {
        if let Some(m) = crate::history::decode(rec) {
            self.history.restore(m);
        }
    }

    /// Boot: an offline queue entry that storage shows as deleted afterwards.
    pub fn forget_queued(&mut self, seq: u32) {
        if self.queue.iter().any(|q| q.seq == seq) {
            let mut kept: heapless::Deque<Queued, OFFLINE_QUEUE> = heapless::Deque::new();
            for q in self.queue.iter().filter(|q| q.seq != seq) {
                let _ = kept.push_back(*q);
            }
            self.queue = kept;
        }
    }

    /// Boot: an offline queue entry read back from storage (any order).
    pub fn restore_queued(&mut self, seq: u32, entry: &[u8]) {
        let (Some(link), Some(&code)) = (entry.get(..4), entry.get(4)) else {
            return;
        };
        let q = Queued {
            seq,
            channel: matches!(code, 8 | 17 | 27),
            hist: u32::from_le_bytes(fixed(link)),
        };
        if self.queue.iter().any(|x| x.seq == seq) {
            return;
        }
        let mut sorted: heapless::Vec<Queued, OFFLINE_QUEUE> = self.queue.iter().copied().collect();
        if sorted.push(q).is_err() {
            return;
        }
        sorted.sort_unstable_by_key(|x| x.seq);
        self.queue.clear();
        for x in sorted {
            let _ = self.queue.push_back(x);
        }
        self.queue_seq = self.queue_seq.max(seq.wrapping_add(1));
    }

    pub fn queued_messages(&self) -> usize {
        self.queue.len()
    }

    pub fn set_connected(&mut self, c: bool) {
        self.connected = c;
    }

    fn reply(&mut self, r: &Response<'_>) {
        if let Some(f) = frame_of(r)
            && self.out.push_back(f).is_err()
        {
            // Queue full: the frame is lost (`writeFrame` on a saturated transport).
        }
    }

    fn err(&mut self, code: u8) {
        self.reply(&Response::Err(code));
    }

    fn done(&mut self, r: SetResult) {
        match r {
            Ok(()) => self.ok(),
            Err(e) => self.err(e),
        }
    }

    fn ok(&mut self) {
        self.reply(&Response::Ok);
    }

    /// Notification: only if a transport is connected, otherwise lost (like in C++).
    fn push(&mut self, r: &Response<'_>) {
        if self.connected {
            self.reply(r);
        }
    }

    fn mark_contacts(&mut self, now: Instant) {
        if self.contacts_dirty.is_none() {
            self.contacts_dirty = Some(now + CONTACTS_SAVE_DELAY);
        }
    }

    fn save_prefs(&mut self, st: &mut impl Storage) {
        st.save_prefs(&encode_prefs(&self.node.prefs));
    }

    /// Queues a message (`addToOfflineQueue`): when full, the oldest channel message is dropped;
    /// if there is none, the new one is lost. `MSG_WAITING` is sent in every case. The frame
    /// goes to storage, the index stays here; `hist` links it to the history.
    fn enqueue(&mut self, f: Frame, hist: u32, st: &mut impl Storage) {
        if self.queue.is_full()
            && let Some(pos) = self.queue.iter().position(|q| q.channel)
        {
            let mut kept: heapless::Deque<Queued, OFFLINE_QUEUE> = heapless::Deque::new();
            for (i, x) in self.queue.iter().enumerate() {
                if i == pos {
                    st.remove_queued(x.seq);
                } else {
                    let _ = kept.push_back(*x);
                }
            }
            self.queue = kept;
        }
        if !self.queue.is_full() {
            let seq = self.queue_seq;
            self.queue_seq = self.queue_seq.wrapping_add(1).max(1);
            let mut entry: heapless::Vec<u8, QUEUED_MAX> = heapless::Vec::new();
            let _ = entry.extend_from_slice(&hist.to_le_bytes());
            let _ = entry.extend_from_slice(&f);
            st.put_queued(seq, &entry);
            let _ = self.queue.push_back(Queued {
                seq,
                channel: matches!(f.first(), Some(8 | 17 | 27)),
                hist,
            });
        }
        self.push(&Response::MsgWaiting);
    }

    /// `SYNC_NEXT_MESSAGE`: the oldest queued frame, read back from storage. A message fetched
    /// by the app is read on the screen too.
    fn sync_next(&mut self, st: &mut impl Storage) {
        while let Some(q) = self.queue.pop_front() {
            let mut buf = [0u8; QUEUED_MAX];
            let got = st.get_queued(q.seq, &mut buf);
            st.remove_queued(q.seq);
            if q.hist != 0
                && let Some(slot) = self.history.set_status(q.hist, Status::Read)
            {
                self.save_history_slot(slot, st);
                self.notify(UiEvent::Status { seq: q.hist });
            }
            if let Some(n) = got
                && let Some(frame) = buf.get(4..n)
                && let Ok(f) = Frame::from_slice(frame)
            {
                let _ = self.out.push_back(f);
                return;
            }
        }
        self.reply(&Response::NoMoreMessages);
    }

    /// Stores a received message in the history; returns its sequence number.
    #[allow(clippy::too_many_arguments)]
    fn remember(
        &mut self,
        now: Instant,
        peer: Peer,
        ts: u32,
        path_len: u8,
        snr4: i8,
        text: &[u8],
        st: &mut impl Storage,
    ) -> u32 {
        let text = text.get(..text.len().min(HISTORY_TEXT)).unwrap_or(&[]);
        let (seq, slot) = self.history.push(Msg {
            seq: 0,
            peer,
            outgoing: false,
            ts,
            at: self.node.time(now),
            status: Status::Unread,
            path_len,
            snr4,
            text: heapless::Vec::from_slice(text).unwrap_or_default(),
        });
        self.save_history_slot(slot, st);
        self.notify(UiEvent::Message {
            seq,
            peer,
            outgoing: false,
        });
        seq
    }

    // ------------------------------------------------------------------ node events

    fn pump(&mut self, now: Instant, rng: &mut impl Rng, st: &mut impl Storage) {
        while let Some(e) = self.node.poll_event() {
            self.on_event(now, &e, rng, st);
            #[cfg(feature = "tap")]
            if !matches!(e, Event::RxLog { .. }) {
                if self.tap.is_full() {
                    let _ = self.tap.pop_front();
                }
                let _ = self.tap.push_back(e);
            }
        }
    }

    fn on_event(&mut self, now: Instant, e: &Event, rng: &mut impl Rng, st: &mut impl Storage) {
        match e {
            Event::RxLog { snr4, rssi, raw } => {
                let at = self.node.time(now);
                self.packets.push(LoggedPacket::of(raw, at, *snr4, *rssi));
                if raw.len() + 3 <= FRAME_MAX {
                    self.push(&Response::LogRxData {
                        snr4: *snr4,
                        rssi: *rssi,
                        raw,
                    });
                }
            }
            Event::ContactMsg {
                key,
                snr4,
                path_len,
                txt_type,
                ts,
                author,
                text,
            } => {
                // Plain and signed texts go to the screen's history; CLI data (type 1) does not.
                let hist = if *txt_type == 1 {
                    0
                } else {
                    self.remember(
                        now,
                        Peer::Contact(prefix6(key)),
                        *ts,
                        *path_len,
                        *snr4,
                        text,
                        st,
                    )
                };
                let mut body: heapless::Vec<u8, FRAME_MAX> = heapless::Vec::new();
                if let Some(a) = author {
                    let _ = body.extend_from_slice(a);
                }
                let _ = body.extend_from_slice(text);
                let room = FRAME_MAX - if self.app_target_ver >= 3 { 16 } else { 13 };
                let body = body.get(..body.len().min(room)).unwrap_or(&[]);
                let r = if self.app_target_ver >= 3 {
                    Response::ContactMsg {
                        snr4: *snr4,
                        reserved: [0; 2],
                        prefix: prefix6(key),
                        path_len: *path_len,
                        txt_type: *txt_type,
                        timestamp: *ts,
                        text: body,
                    }
                } else {
                    Response::ContactMsgLegacy {
                        prefix: prefix6(key),
                        path_len: *path_len,
                        txt_type: *txt_type,
                        timestamp: *ts,
                        text: body,
                    }
                };
                if let Some(f) = frame_of(&r) {
                    self.enqueue(f, hist, st);
                }
            }
            Event::ChannelMsg {
                idx,
                snr4,
                path_len,
                txt_type,
                ts,
                text,
            } => {
                let hist = self.remember(now, Peer::Channel(*idx), *ts, *path_len, *snr4, text, st);
                let room = FRAME_MAX - if self.app_target_ver >= 3 { 11 } else { 8 };
                let body = text.get(..text.len().min(room)).unwrap_or(&[]);
                let r = if self.app_target_ver >= 3 {
                    Response::ChannelMsg {
                        snr4: *snr4,
                        reserved: [0; 2],
                        idx: *idx,
                        path_len: *path_len,
                        txt_type: *txt_type,
                        timestamp: *ts,
                        text: body,
                    }
                } else {
                    Response::ChannelMsgLegacy {
                        idx: *idx,
                        path_len: *path_len,
                        txt_type: *txt_type,
                        timestamp: *ts,
                        text: body,
                    }
                };
                if let Some(f) = frame_of(&r) {
                    self.enqueue(f, hist, st);
                }
            }
            Event::ChannelData {
                idx,
                snr4,
                path_len,
                data_type,
                data,
            } => {
                let r = Response::ChannelData {
                    snr4: *snr4,
                    reserved: [0; 2],
                    idx: *idx,
                    path_len: *path_len,
                    data_type: *data_type,
                    data,
                };
                if let Some(f) = frame_of(&r) {
                    self.enqueue(f, 0, st);
                }
            }
            Event::Advert { key, raw } => {
                st.put_blob(key, raw);
                self.mark_contacts(now);
                self.push(&Response::Advert { key: *key });
                self.notify(UiEvent::Contacts);
            }
            Event::NewAdvert { contact } => self.push(&Response::NewAdvert(rcontact(contact))),
            Event::ContactOverwritten { key } => {
                st.remove_blob(key);
                self.push(&Response::ContactDeleted { key: *key });
                self.notify(UiEvent::Contacts);
            }
            Event::ContactsFull => self.push(&Response::ContactsFull),
            Event::AdvertHeard {
                key,
                kind,
                path_len,
                path,
                snr4,
                name,
                location,
            } => {
                let prefix: [u8; 7] = fixed(key);
                let recv_ts = self.node.time(now);
                self.heard.note(HeardNode {
                    key: *key,
                    name: *name,
                    kind: *kind,
                    hops: PathLen::from_byte(*path_len).map(|p| p.hops()).unwrap_or(0),
                    snr4: *snr4,
                    at: recv_ts,
                    location: *location,
                    count: 0,
                });
                self.notify(UiEvent::Heard);
                let entry = AdvertPath {
                    prefix,
                    recv_ts,
                    path_len: *path_len,
                    path: path.clone(),
                };
                if let Some(pos) = self.advert_paths.iter().position(|a| a.prefix == prefix) {
                    let mut kept: heapless::Deque<AdvertPath, 16> = heapless::Deque::new();
                    for (i, a) in self.advert_paths.iter().enumerate() {
                        if i != pos {
                            let _ = kept.push_back(a.clone());
                        }
                    }
                    self.advert_paths = kept;
                }
                if self.advert_paths.is_full() {
                    let _ = self.advert_paths.pop_front();
                }
                let _ = self.advert_paths.push_back(entry);
            }
            Event::PathUpdated { key } => {
                self.mark_contacts(now);
                self.push(&Response::PathUpdated { key: *key });
            }
            Event::SendConfirmed { ack, rtt_ms } => {
                // A message from the screen: the app never sent it.
                if !self.confirm_outgoing(now, u32::from_le_bytes(*ack), st) {
                    self.push(&Response::SendConfirmed {
                        ack: *ack,
                        rtt_ms: *rtt_ms,
                    });
                }
            }
            Event::Response { key, data } => {
                if !self.screen_response(now, key, data) {
                    self.on_response(key, data);
                }
            }
            Event::Request {
                key,
                ts,
                data,
                flood,
                path_len,
                path,
            } => self.on_request(now, key, *ts, data, *flood, *path_len, path, rng),
            Event::RawData { snr4, rssi, data } => self.push(&Response::RawData {
                snr4: *snr4,
                rssi: *rssi,
                reserved: 0xFF,
                payload: data,
            }),
            Event::ControlData {
                snr4,
                rssi,
                path_len,
                data,
            } => self.push(&Response::ControlData {
                snr4: *snr4,
                rssi: *rssi,
                path_len: *path_len,
                payload: data,
            }),
            Event::TraceData {
                path_len,
                flags,
                tag,
                auth,
                hashes,
                snrs,
                final_snr4,
            } => {
                let _ = path_len;
                if self.screen_trace(now, *tag, hashes, snrs, *final_snr4) {
                    return;
                }
                self.push(&Response::TraceData {
                    reserved: 0,
                    flags: *flags,
                    tag: *tag,
                    auth: *auth,
                    hashes,
                    snrs,
                    final_snr4: *final_snr4,
                })
            }
            Event::PathDiscovery {
                key,
                out_len,
                out_path,
                in_len,
                in_path,
            } => self.push(&Response::PathDiscoveryResponse {
                reserved: 0,
                prefix: prefix6(key),
                out_path_len: *out_len,
                out_path,
                in_path_len: *in_len,
                in_path,
            }),
        }
    }

    /// `onContactResponse` (`MyMesh.cpp:694-776`).
    fn on_response(&mut self, key: &[u8; PUB_KEY_LEN], data: &[u8]) {
        let tag = u32::from_le_bytes(fixed(data));
        let key4 = u32::from_le_bytes(fixed(key));
        let len = data.len();
        if self.pending_login != 0 && self.pending_login == key4 {
            self.pending_login = 0;
            if data.get(4..6) == Some(b"OK") {
                self.push(&Response::LoginSuccess {
                    permissions: 0,
                    prefix: prefix6(key),
                    server: None,
                });
            } else if data.get(4) == Some(&0) {
                let b = |i: usize| data.get(i).copied().unwrap_or(0);
                self.push(&Response::LoginSuccess {
                    permissions: b(6),
                    prefix: prefix6(key),
                    server: Some(LoginServer {
                        server_time: tag,
                        acl: b(7),
                        fw_level: b(12),
                    }),
                });
            } else {
                self.push(&Response::LoginFail {
                    reserved: 0,
                    prefix: prefix6(key),
                });
            }
        } else if len > 4 && self.pending_status != 0 && self.pending_status == key4 {
            self.pending_status = 0;
            self.push(&Response::StatusResponse {
                reserved: 0,
                prefix: prefix6(key),
                data: data.get(4..).unwrap_or(&[]),
            });
        } else if len > 4 && self.pending_telemetry != 0 && tag == self.pending_telemetry {
            self.pending_telemetry = 0;
            self.push(&Response::TelemetryResponse {
                reserved: 0,
                prefix: prefix6(key),
                lpp: data.get(4..).unwrap_or(&[]),
            });
        } else if len > 4 && self.pending_req != 0 && tag == self.pending_req {
            self.pending_req = 0;
            self.push(&Response::BinaryResponse {
                reserved: 0,
                tag: le4(tag),
                data: data.get(4..).unwrap_or(&[]),
            });
        } else if len >= 4 {
            // Unexpected response: passed to the app as is.
            self.push(&Response::BinaryResponse {
                reserved: 0,
                tag: fixed(data),
                data: data.get(4..).unwrap_or(&[]),
            });
        }
    }

    fn clear_pending(&mut self) {
        self.pending_login = 0;
        self.pending_status = 0;
        self.pending_telemetry = 0;
        self.pending_req = 0;
        self.node.pending_discovery = 0;
    }

    /// Device telemetry in CayenneLPP: voltage (channel 1, type 116, 0.01 V) and
    /// microcontroller temperature (channel 1, type 103, 0.1 °C).
    fn telemetry(&self) -> heapless::Vec<u8, 16> {
        let mut v = heapless::Vec::new();
        let cv = self.battery_mv / 10;
        let _ = v.extend_from_slice(&[1, 116]);
        let _ = v.extend_from_slice(&cv.to_be_bytes());
        if let Some(t) = self.mcu_temp_deci {
            let _ = v.extend_from_slice(&[1, 103]);
            let _ = v.extend_from_slice(&t.to_be_bytes());
        }
        v
    }

    /// Request from a contact (`onContactRequest`): only telemetry is served, according to the
    /// permissions (`MyMesh.cpp:639-683`).
    #[allow(clippy::too_many_arguments)]
    fn on_request(
        &mut self,
        now: Instant,
        key: &[u8; PUB_KEY_LEN],
        ts: u32,
        data: &[u8],
        flood: bool,
        path_len: u8,
        path: &[u8],
        rng: &mut impl Rng,
    ) {
        let _ = rng;
        if data.first() != Some(&REQ_TYPE_GET_TELEMETRY_DATA) {
            return;
        }
        let Some(slot) = self.node.contacts.find(key) else {
            return;
        };
        let cp = self
            .node
            .contacts
            .get(slot)
            .map(|c| c.flags >> 1)
            .unwrap_or(0);
        let p = &self.node.prefs;
        let mode = |m: u8, bit: u8| match m {
            2 => bit,
            1 => cp & bit,
            _ => 0,
        };
        let mut perms =
            mode(p.telemetry_base, 1) | mode(p.telemetry_loc, 2) | mode(p.telemetry_env, 4);
        perms &= !data.get(1).copied().unwrap_or(0);
        if perms & 1 == 0 {
            return;
        }
        let lpp = self.telemetry();
        let mut reply: heapless::Vec<u8, 32> = heapless::Vec::new();
        let _ = reply.extend_from_slice(&ts.to_le_bytes());
        let _ = reply.extend_from_slice(&lpp);
        let id = self.node.id.clone();
        let Some(c) = self.node.contacts.get_mut(slot) else {
            return;
        };
        let Some(secret) = c.secret(&id) else { return };
        let dest = c.hash();
        if flood {
            let Ok(pl) = PathLen::from_byte(path_len) else {
                return;
            };
            if let Ok(rp) = payload::path_return(
                dest,
                id.hash(),
                &secret,
                pl,
                path,
                Some((mc_air::PayloadType::RESPONSE.value(), &reply)),
                [0; 4],
            ) {
                self.node.send_flood_scoped(rp, SERVER_RESPONSE_DELAY, now);
            }
        } else if let Ok(pkt) = payload::datagram(
            mc_air::PayloadType::RESPONSE,
            dest,
            id.hash(),
            &secret,
            &reply,
        ) {
            let _ = self
                .node
                .send_to_contact(now, slot, pkt, SERVER_RESPONSE_DELAY);
        }
    }

    // ------------------------------------------------------------------ app commands

    fn sent(&mut self, s: Sent) {
        self.reply(&Response::Sent {
            flood: s.flood,
            ack: le4(s.ack),
            timeout_ms: s.timeout_ms,
        });
    }

    fn self_info(&self) -> Response<'_> {
        let p = &self.node.prefs;
        Response::SelfInfo(SelfInfo {
            adv_type: 1,
            tx_power: p.radio.tx_power_dbm.to_le_bytes()[0],
            max_tx_power: self.info.max_tx_power.to_le_bytes()[0],
            key: *self.node.id.pub_key(),
            lat: p.lat,
            lon: p.lon,
            multi_acks: p.multi_acks,
            advert_loc_policy: p.advert_loc_policy,
            telemetry: (p.telemetry_env << 4) | (p.telemetry_loc << 2) | p.telemetry_base,
            manual_add: p.manual_add_contacts,
            freq_khz: p.radio.freq_khz,
            bw_hz: p.radio.bw_hz,
            sf: p.radio.sf,
            cr: p.radio.cr,
            node_name: name_str(&p.name),
        })
    }

    fn device_info(&self) -> Response<'_> {
        Response::DeviceInfo(DeviceInfo {
            fw_ver: FIRMWARE_VER_CODE,
            max_contacts_half: u8::try_from(MAX_CONTACTS / 2).unwrap_or(0xFF),
            max_channels: u8::try_from(MAX_CHANNELS).unwrap_or(0xFF),
            ble_pin: self.node.prefs.ble_pin,
            build_date: fixed(self.info.build_date.as_bytes()),
            manufacturer: fixed(self.info.manufacturer.as_bytes()),
            version: fixed(self.info.version.as_bytes()),
            repeat: u8::from(self.node.prefs.repeat),
            path_hash_mode: self.node.prefs.path_hash_mode,
            ext: &[],
        })
    }

    /// A frame from the app (`handleCmdFrame`).
    pub fn on_app_frame(
        &mut self,
        now: Instant,
        frame: &[u8],
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) {
        match Command::decode_device(frame) {
            Err(DeviceReject::Ignored) => {}
            Err(DeviceReject::Err(c)) => self.err(c),
            Ok(cmd) => self.handle(now, cmd, rng, st),
        }
        self.pump(now, rng, st);
    }

    #[allow(clippy::too_many_lines)]
    fn handle(
        &mut self,
        now: Instant,
        cmd: Command<'_>,
        rng: &mut impl Rng,
        st: &mut impl Storage,
    ) {
        match cmd {
            Command::DeviceQuery { app_ver } => {
                self.app_target_ver = app_ver;
                let r = self.device_info();
                let f = frame_of(&r);
                if let Some(f) = f {
                    let _ = self.out.push_back(f);
                }
            }
            Command::AppStart { .. } => {
                self.iter = None;
                let f = frame_of(&self.self_info());
                if let Some(f) = f {
                    let _ = self.out.push_back(f);
                }
            }
            Command::SendTxtMsg {
                txt_type,
                attempt,
                timestamp,
                prefix,
                text,
            } => {
                match self
                    .node
                    .send_text(now, &prefix, txt_type, attempt, timestamp, text)
                {
                    Ok(s) => self.sent(s),
                    Err(e) => self.err(err_of(e)),
                }
            }
            Command::SendChannelTxtMsg {
                txt_type,
                idx,
                timestamp,
                text,
            } => {
                if txt_type != 0 {
                    return self.err(err::UNSUPPORTED);
                }
                if usize::from(idx) >= MAX_CHANNELS {
                    return self.err(err::NOT_FOUND);
                }
                match self
                    .node
                    .send_channel_text(now, idx, timestamp, name_str(text))
                {
                    Ok(()) => self.ok(),
                    Err(_) => self.err(err::NOT_FOUND),
                }
            }
            Command::GetContacts { since } => {
                if self.iter.is_some() {
                    return self.err(err::BAD_STATE);
                }
                let total = u32::try_from(self.node.contacts.list.len()).unwrap_or(0);
                self.reply(&Response::ContactsStart { total });
                self.iter = Some(Iter {
                    since: since.unwrap_or(0),
                    next: 0,
                    most_recent: 0,
                });
            }
            Command::GetDeviceTime => {
                let t = self.node.time(now);
                self.reply(&Response::CurrTime { unix_s: t });
            }
            Command::SetDeviceTime { unix_s } => {
                if unix_s >= self.node.time(now) {
                    self.node.rtc.set(now, unix_s);
                    self.ok();
                } else {
                    self.err(err::ILLEGAL_ARG);
                }
            }
            Command::SendSelfAdvert { flood } => {
                match self.node.send_self_advert(now, flood == Some(true)) {
                    Ok(()) => self.ok(),
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::SetAdvertName { name } => {
                self.set_name(name, st);
                self.ok();
            }
            Command::AddUpdateContact {
                key,
                kind,
                flags,
                path_len,
                path,
                name,
                last_advert,
                lat_lon,
                lastmod,
            } => {
                let epoch = self.node.time(now);
                let slot = match self.node.contacts.find(&key) {
                    Some(Slot::List(i)) => Some(i),
                    _ => {
                        let mut c = Contact::new(key);
                        c.sync_since = 0;
                        if self.node.contacts.list.push(c).is_err() {
                            return self.err(err::TABLE_FULL);
                        }
                        Some(self.node.contacts.list.len() - 1)
                    }
                };
                if let Some(c) = slot.and_then(|i| self.node.contacts.list.get_mut(i)) {
                    c.kind = kind;
                    c.flags = flags;
                    c.out_path_len = path_len;
                    c.out_path = path;
                    c.name = name;
                    c.last_advert = last_advert;
                    if let Some((la, lo)) = lat_lon {
                        c.lat = la;
                        c.lon = lo;
                    }
                    c.lastmod = lastmod.unwrap_or(epoch);
                    c.forget_secret();
                }
                self.mark_contacts(now);
                self.ok();
            }
            Command::SyncNextMessage => self.sync_next(st),
            Command::SetRadioParams {
                freq_khz,
                bw_hz,
                sf,
                cr,
                repeat,
            } => {
                let r = self.set_radio(freq_khz, bw_hz, sf, cr, repeat.unwrap_or(0) != 0, st);
                self.done(r);
            }
            Command::SetRadioTxPower { dbm } => {
                let r = self.set_tx_power(dbm, st);
                self.done(r);
            }
            Command::ResetPath { key } => {
                let r = self.reset_contact_path(now, &key);
                self.done(r);
            }
            Command::SetAdvertLatLon { lat, lon, .. } => {
                let r = self.set_location(lat, lon, st);
                self.done(r);
            }
            Command::RemoveContact { key } => {
                let r = self.remove_contact(now, &key, st);
                self.done(r);
            }
            Command::ShareContact { key } => {
                if self.node.contacts.find(&key).is_none() {
                    return self.err(err::NOT_FOUND);
                }
                let mut buf = [0u8; 255];
                match st.get_blob(&key, &mut buf).and_then(|n| buf.get(..n)) {
                    Some(raw) => {
                        let Ok(p) = mc_air::Packet::decode(raw) else {
                            return self.err(err::TABLE_FULL);
                        };
                        let Ok(pkt) = mc_air::PacketBuf::from_packet(&p) else {
                            return self.err(err::TABLE_FULL);
                        };
                        self.node.send_zero_hop(pkt, Some([0, 0]), 0, now);
                        self.ok();
                    }
                    None => self.err(err::TABLE_FULL),
                }
            }
            Command::ExportContact { key } => match key {
                None => {
                    let Ok(mut pkt) = self.node.self_advert(now) else {
                        return self.err(err::TABLE_FULL);
                    };
                    pkt.header.route = mc_air::Route::Flood;
                    let mut buf = [0u8; 255];
                    match pkt.encode(&mut buf) {
                        Ok(n) => self.reply(&Response::ExportContact {
                            raw: buf.get(..n).unwrap_or(&[]),
                        }),
                        Err(_) => self.err(err::TABLE_FULL),
                    }
                }
                Some(k) => {
                    let mut buf = [0u8; 255];
                    match (self.node.contacts.find(&k), st.get_blob(&k, &mut buf)) {
                        (Some(_), Some(n)) => self.reply(&Response::ExportContact {
                            raw: buf.get(..n).unwrap_or(&[]),
                        }),
                        _ => self.err(err::NOT_FOUND),
                    }
                }
            },
            Command::ImportContact { raw } => {
                let is_advert = mc_air::Packet::decode(raw)
                    .is_ok_and(|p| p.header.ptype == mc_air::PayloadType::ADVERT);
                if !is_advert {
                    return self.err(err::ILLEGAL_ARG);
                }
                self.ok();
                let _ = self.node.import_advert(now, raw, rng);
            }
            Command::Reboot => {
                self.flush_contacts(st);
                let _ = self.actions.push_back(Action::Reboot);
            }
            Command::GetBattAndStorage => {
                let (used_kb, total_kb) = st.usage_kb();
                self.reply(&Response::BattAndStorage {
                    battery_mv: self.battery_mv,
                    used_kb,
                    total_kb,
                });
            }
            Command::SetTuningParams {
                rx_delay,
                airtime_factor,
            } => {
                self.set_tuning(rx_delay, airtime_factor, st);
                self.ok();
            }
            Command::GetTuningParams => {
                let p = &self.node.prefs;
                self.reply(&Response::TuningParams {
                    rx_delay: p.rx_delay_base_milli,
                    airtime_factor: p.airtime_factor_milli,
                });
            }
            Command::ExportPrivateKey => {
                let key = *self.node.id.private_key();
                self.reply(&Response::PrivateKey { key });
            }
            Command::ImportPrivateKey { key } => {
                let id = LocalIdentity::from_private(&key);
                // C++ checks: public key not reserved, ECDH self-test (`Identity.cpp:67-109`).
                let probe = LocalIdentity::from_seed(&[0x5A; 32]);
                let ok = !id.is_reserved()
                    && id.shared_secret(probe.pub_key()).is_some()
                    && id.shared_secret(probe.pub_key()) == probe.shared_secret(id.pub_key());
                if !ok {
                    return self.err(err::ILLEGAL_ARG);
                }
                st.save_identity(id.private_key(), id.pub_key());
                self.node.set_identity(id);
                self.ok();
            }
            Command::SendRawData {
                path_len,
                path,
                payload,
            } => match self.node.send_raw_data(now, path_len, path, payload) {
                Ok(()) => self.ok(),
                Err(SendError::Unsupported) => self.err(err::UNSUPPORTED),
                Err(_) => self.err(err::TABLE_FULL),
            },
            Command::SendLogin { key, password } => {
                let Some(slot) = self.node.contacts.find(&key) else {
                    return self.err(err::NOT_FOUND);
                };
                match self.node.send_login(now, slot, password) {
                    Ok(s) => {
                        self.clear_pending();
                        self.pending_login = s.ack;
                        self.sent(s);
                    }
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::SendStatusReq { key } => {
                let Some(slot) = self.node.contacts.find(&key) else {
                    return self.err(err::NOT_FOUND);
                };
                match self.node.send_typed_request(now, slot, 0x01, rng) {
                    Ok(s) => {
                        self.clear_pending();
                        self.pending_status = u32::from_le_bytes(fixed(&key));
                        self.sent(s);
                    }
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::HasConnection { .. } => self.err(err::NOT_FOUND),
            Command::Logout { .. } => self.ok(),
            Command::GetContactByKey { key } => match self
                .node
                .contacts
                .find(&key)
                .and_then(|s| self.node.contacts.get(s))
            {
                Some(c) => {
                    let r = rcontact(c);
                    self.reply(&Response::Contact(r));
                }
                None => self.err(err::NOT_FOUND),
            },
            Command::GetChannel { idx } => match self.node.channels.0.get(usize::from(idx)) {
                Some(c) => {
                    let (name, secret) = (c.name, fixed::<16>(&c.secret.0));
                    self.reply(&Response::ChannelInfo { idx, name, secret });
                }
                None => self.err(err::NOT_FOUND),
            },
            Command::SetChannel { idx, name, secret } => {
                let r = self.set_channel(idx, name, secret, st);
                self.done(r);
            }
            Command::SignStart => {
                self.sign = Some(heapless::Vec::new());
                self.reply(&Response::SignStart {
                    reserved: 0,
                    max_len: u32::try_from(SIGN_MAX).unwrap_or(0),
                });
            }
            Command::SignData { data } => match self.sign.as_mut() {
                None => self.err(err::BAD_STATE),
                Some(buf) => {
                    if buf.extend_from_slice(data).is_err() {
                        self.err(err::TABLE_FULL);
                    } else {
                        self.ok();
                    }
                }
            },
            Command::SignFinish => match self.sign.take() {
                None => self.err(err::BAD_STATE),
                Some(buf) => match self.node.id.sign(&[&buf]) {
                    Ok(signature) => self.reply(&Response::Signature { signature }),
                    Err(_) => self.err(err::BAD_STATE),
                },
            },
            Command::SendTracePath {
                tag,
                auth,
                flags,
                path,
            } => match self.node.send_trace(now, tag, auth, flags, path) {
                Ok(timeout_ms) => self.reply(&Response::Sent {
                    flood: false,
                    ack: le4(tag),
                    timeout_ms,
                }),
                Err(_) => self.err(err::TABLE_FULL),
            },
            Command::SetDevicePin { pin } => {
                let r = self.set_pin(pin, st);
                self.done(r);
            }
            Command::SetOtherParams {
                manual_add,
                telemetry,
                advert_loc_policy,
                multi_acks,
            } => {
                self.set_other_params(manual_add, telemetry, advert_loc_policy, multi_acks, st);
                self.ok();
            }
            Command::SendTelemetryReq { key, .. } => match key {
                None => {
                    let lpp = self.telemetry();
                    let me = prefix6(self.node.id.pub_key());
                    self.reply(&Response::TelemetryResponse {
                        reserved: 0,
                        prefix: me,
                        lpp: &lpp,
                    });
                }
                Some(k) => {
                    let Some(slot) = self.node.contacts.find(&k) else {
                        return self.err(err::NOT_FOUND);
                    };
                    match self
                        .node
                        .send_typed_request(now, slot, REQ_TYPE_GET_TELEMETRY_DATA, rng)
                    {
                        Ok(s) => {
                            self.clear_pending();
                            self.pending_telemetry = s.ack;
                            self.sent(s);
                        }
                        Err(_) => self.err(err::TABLE_FULL),
                    }
                }
            },
            Command::GetCustomVars => {
                let mut v: heapless::String<64> = heapless::String::new();
                if self.info.has_gps {
                    let _ = core::fmt::write(
                        &mut v,
                        format_args!("gps:{}", u8::from(self.node.prefs.gps_enabled)),
                    );
                }
                self.reply(&Response::CustomVars { vars: v.as_bytes() });
            }
            Command::SetCustomVar { var } => {
                let s = core::str::from_utf8(var).unwrap_or("");
                match s.split_once(':') {
                    Some(("gps", v)) if self.info.has_gps => {
                        let r = self.set_gps(v.trim() == "1", st);
                        self.done(r);
                    }
                    Some(("gps_interval", v)) if self.info.has_gps => {
                        self.gps_interval = v.trim().parse().unwrap_or(0);
                        self.ok();
                    }
                    _ => self.err(err::ILLEGAL_ARG),
                }
            }
            Command::GetAdvertPath { key, .. } => {
                let prefix: [u8; 7] = fixed(&key);
                match self
                    .advert_paths
                    .iter()
                    .find(|a| a.prefix == prefix)
                    .cloned()
                {
                    Some(a) => self.reply(&Response::AdvertPath {
                        recv_ts: a.recv_ts,
                        path_len: a.path_len,
                        path: &a.path,
                    }),
                    None => self.err(err::NOT_FOUND),
                }
            }
            Command::SendBinaryReq { key, data } => {
                let Some(slot) = self.node.contacts.find(&key) else {
                    return self.err(err::NOT_FOUND);
                };
                match self.node.send_request(now, slot, data) {
                    Ok(s) => {
                        self.clear_pending();
                        self.pending_req = s.ack;
                        self.sent(s);
                    }
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::FactoryReset => {
                let _ = self.actions.push_back(Action::FactoryReset);
                self.ok();
            }
            Command::SendPathDiscoveryReq { key } => {
                let Some(slot) = self.node.contacts.find(&key) else {
                    return self.err(err::NOT_FOUND);
                };
                let mut req = [0u8; 9];
                req[0] = REQ_TYPE_GET_TELEMETRY_DATA;
                req[1] = !1u8;
                let r = rng.next_u32().to_le_bytes();
                for (o, b) in req.iter_mut().skip(5).zip(r) {
                    *o = b;
                }
                // Flood forced for the duration of the send.
                let saved = self
                    .node
                    .contacts
                    .get(slot)
                    .map(|c| c.out_path_len)
                    .unwrap_or(PATH_UNKNOWN);
                if let Some(c) = self.node.contacts.get_mut(slot) {
                    c.out_path_len = PATH_UNKNOWN;
                }
                let res = self.node.send_request(now, slot, &req);
                if let Some(c) = self.node.contacts.get_mut(slot) {
                    c.out_path_len = saved;
                }
                match res {
                    Ok(s) => {
                        self.clear_pending();
                        self.node.pending_discovery = s.ack;
                        self.sent(s);
                    }
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::SetFloodScopeKey { scope } => {
                use mc_proto::command::FloodScopeKey;
                match scope {
                    FloodScopeKey::Clear => {
                        self.node.send_scope = [0; 16];
                        self.node.send_unscoped = false;
                    }
                    FloodScopeKey::Key(k) => {
                        self.node.send_scope = k;
                        self.node.send_unscoped = false;
                    }
                    FloodScopeKey::Unscoped => self.node.send_unscoped = true,
                }
                self.ok();
            }
            Command::SendControlData { data } => match self.node.send_control(now, data) {
                Ok(()) => self.ok(),
                Err(_) => self.err(err::TABLE_FULL),
            },
            Command::GetStats { kind } => {
                let st_ = self.node.disp.stats;
                let s = match kind {
                    0 => Stats::Core {
                        battery_mv: self.battery_mv,
                        uptime_s: u32::try_from(
                            now.saturating_duration_since(self.node.boot).as_millis() / 1000,
                        )
                        .unwrap_or(0),
                        err_flags: st_.err_flags,
                        queue_len: u8::try_from(self.node.disp.len()).unwrap_or(0xFF),
                    },
                    1 => Stats::Radio {
                        noise_floor: self.noise_floor,
                        last_rssi: i8::try_from(self.node.last_rssi.clamp(-128, 127)).unwrap_or(0),
                        last_snr4: self.node.last_snr4,
                        tx_air_s: u32::try_from(st_.tx_airtime_ms / 1000).unwrap_or(0),
                        rx_air_s: u32::try_from(st_.rx_airtime_ms / 1000).unwrap_or(0),
                    },
                    2 => Stats::Packets {
                        recv: st_.recv_flood.wrapping_add(st_.recv_direct),
                        sent: st_.sent_flood.wrapping_add(st_.sent_direct),
                        sent_flood: st_.sent_flood,
                        sent_direct: st_.sent_direct,
                        recv_flood: st_.recv_flood,
                        recv_direct: st_.recv_direct,
                        recv_errors: st_.recv_errors,
                    },
                    _ => return self.err(err::ILLEGAL_ARG),
                };
                self.reply(&Response::Stats(s));
            }
            Command::SendAnonReq { key, data } => {
                let slot = match self.node.contacts.find(&key) {
                    Some(s) => s,
                    None => self.node.temp_contact(now, &key),
                };
                match self.node.send_anon_req(now, slot, data) {
                    Ok(s) => {
                        self.clear_pending();
                        self.pending_req = s.ack;
                        self.sent(s);
                    }
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::SetAutoaddConfig { config, max_hops } => {
                self.set_autoadd(config, max_hops, st);
                self.ok();
            }
            Command::GetAutoaddConfig => {
                let p = &self.node.prefs;
                self.reply(&Response::AutoaddConfig {
                    config: p.autoadd_config,
                    max_hops: p.autoadd_max_hops,
                });
            }
            Command::GetAllowedRepeatFreq => {
                let mut b = [0u8; 24];
                for (chunk, f) in b.chunks_mut(8).zip(REPEAT_FREQS) {
                    for (o, x) in chunk
                        .iter_mut()
                        .zip(f.to_le_bytes().into_iter().chain(f.to_le_bytes()))
                    {
                        *o = x;
                    }
                }
                self.reply(&Response::AllowedRepeatFreq { ranges: &b });
            }
            Command::SetPathHashMode { mode } => {
                let r = self.set_path_hash_mode(mode, st);
                self.done(r);
            }
            Command::SendChannelData {
                idx,
                path,
                data_type,
                data,
            } => {
                if usize::from(idx) >= MAX_CHANNELS {
                    return self.err(err::NOT_FOUND);
                }
                if data_type == 0
                    || data.len() > 165
                    || path.is_some_and(|(l, _)| PathLen::from_byte(l).is_err())
                {
                    return self.err(err::ILLEGAL_ARG);
                }
                match self.node.send_channel_data(now, idx, path, data_type, data) {
                    Ok(()) => self.ok(),
                    Err(SendError::NotFound) => self.err(err::NOT_FOUND),
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::SetDefaultFloodScope { scope } => {
                match scope {
                    Some((name, key)) => {
                        let n = name_str(&name).len();
                        if n == 0 || n > 30 {
                            return self.err(err::ILLEGAL_ARG);
                        }
                        self.node.prefs.default_scope_name = name;
                        self.node.prefs.default_scope_key = key;
                    }
                    None => {
                        self.node.prefs.default_scope_name = [0; 31];
                        self.node.prefs.default_scope_key = [0; 16];
                    }
                }
                self.save_prefs(st);
                self.ok();
            }
            Command::GetDefaultFloodScope => {
                let p = &self.node.prefs;
                let scope = (p.default_scope_key != [0; 16])
                    .then_some((p.default_scope_name, p.default_scope_key));
                self.reply(&Response::DefaultFloodScope { scope });
            }
            Command::SendRawPacket { priority, raw } => {
                match self.node.send_raw_packet(now, priority, raw) {
                    Ok(()) => self.ok(),
                    Err(SendError::Unsupported) => self.err(err::ILLEGAL_ARG),
                    Err(_) => self.err(err::TABLE_FULL),
                }
            }
            Command::RunCliCommand { cmd } => {
                let mut reply: heapless::String<160> = heapless::String::new();
                self.cli(now, cmd, &mut reply, st);
                self.reply(&Response::CliReply {
                    text: reply.as_bytes(),
                });
            }
        }
    }

    /// Device console (`RUN_CLI_COMMAND`), a subset of `CommonRadioPrefs.cpp`.
    /// A `"xx|"` prefix is echoed back as is. Texts in English.
    fn cli(
        &mut self,
        now: Instant,
        cmd: &[u8],
        out: &mut heapless::String<160>,
        st: &mut impl Storage,
    ) {
        let s = core::str::from_utf8(name_str(cmd)).unwrap_or("").trim();
        let (tag, s) = match s.get(..3) {
            Some(t) if t.ends_with('|') => (t, s.get(3..).unwrap_or("")),
            _ => ("", s),
        };
        let _ = out.push_str(tag);
        let p = &mut self.node.prefs;
        let w = |out: &mut heapless::String<160>, a: core::fmt::Arguments<'_>| {
            let _ = core::fmt::write(out, a);
        };
        match s.split_once(' ').unwrap_or((s, "")) {
            ("ver", _) => w(out, format_args!("{} (oxymesh)", self.info.version)),
            ("board", _) => w(out, format_args!("{}", self.info.manufacturer)),
            ("clock", _) => w(out, format_args!("{}", self.node.rtc.now(now))),
            ("get", "name") => w(
                out,
                format_args!(
                    "> {}",
                    core::str::from_utf8(name_str(&p.name)).unwrap_or("?")
                ),
            ),
            ("get", "radio") => w(
                out,
                format_args!(
                    "> {}.{:03},{}.{},{},{}",
                    p.radio.freq_khz / 1000,
                    p.radio.freq_khz % 1000,
                    p.radio.bw_hz / 1000,
                    (p.radio.bw_hz % 1000) / 100,
                    p.radio.sf,
                    p.radio.cr
                ),
            ),
            ("get", "tx") => w(out, format_args!("> {}", p.radio.tx_power_dbm)),
            ("set", rest) if rest.starts_with("name ") => {
                self.set_name(rest.get(5..).unwrap_or("").as_bytes(), st);
                let _ = out.push_str("OK");
            }
            ("reboot", _) => {
                let _ = self.actions.push_back(Action::Reboot);
                let _ = out.push_str("OK");
            }
            _ => {
                let _ = out.push_str("Unknown command");
            }
        }
    }
}
