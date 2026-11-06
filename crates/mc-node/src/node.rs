//! The node: state, settings, events and interface with the runner.

use mc_air::crypto::{LocalIdentity, PUB_KEY_LEN};
use mc_air::{PacketBuf, PathLen, PayloadType, Route, packet::PATH_MAX};
use mc_core::{Duration, FRAME_MAX, Instant};
use rand_core::Rng;

use crate::clock::Rtc;
use crate::contacts::{Channels, Contacts};
use crate::dispatch::{Dispatcher, RadioParams, TxFrame, next_int};
use crate::tables::Seen;

/// Longest message text (`MAX_TEXT_LEN`).
pub const TEXT_MAX: usize = 160;
/// Expected acks (`EXPECTED_ACK_TABLE_SIZE`).
pub const ACK_TABLE: usize = 8;

/// Persistent settings (`NodePrefs.h`). The C++ `float` factors are stored × 1000.
#[derive(Debug, Clone)]
pub struct Prefs {
    pub name: [u8; 32],
    pub lat: i32,
    pub lon: i32,
    pub radio: RadioParams,
    pub airtime_factor_milli: u32,
    pub tx_delay_milli: u32,
    pub direct_tx_delay_milli: u32,
    pub rx_delay_base_milli: u32,
    pub multi_acks: u8,
    pub manual_add_contacts: u8,
    pub autoadd_config: u8,
    pub autoadd_max_hops: u8,
    pub telemetry_base: u8,
    pub telemetry_loc: u8,
    pub telemetry_env: u8,
    pub advert_loc_policy: u8,
    pub ble_pin: u32,
    pub repeat: bool,
    pub path_hash_mode: u8,
    /// Default scope: name (0x00-terminated) and key; all-zero key = none.
    pub default_scope_name: [u8; 31],
    pub default_scope_key: [u8; 16],
    pub interference_threshold: i8,
    pub cad_enabled: bool,
    pub gps_enabled: bool,
    pub buzzer_quiet: bool,
}

impl Prefs {
    /// Zeroed settings, for [`Node::empty`].
    pub const ZERO: Self = Self {
        name: [0; 32],
        lat: 0,
        lon: 0,
        radio: RadioParams {
            freq_khz: 869_618,
            bw_hz: 62_500,
            sf: 8,
            cr: 5,
            tx_power_dbm: 22,
        },
        airtime_factor_milli: 1000,
        tx_delay_milli: 500,
        direct_tx_delay_milli: 200,
        rx_delay_base_milli: 0,
        multi_acks: 0,
        manual_add_contacts: 0,
        autoadd_config: 0,
        autoadd_max_hops: 0,
        telemetry_base: 0,
        telemetry_loc: 0,
        telemetry_env: 0,
        advert_loc_policy: 0,
        ble_pin: 0,
        repeat: false,
        path_hash_mode: 0,
        default_scope_name: [0; 31],
        default_scope_key: [0; 16],
        interference_threshold: 0,
        cad_enabled: false,
        gps_enabled: false,
        buzzer_quiet: false,
    };

    /// Factory settings of the M5 build (`MyMesh.cpp:916-1007`): name = first 4 bytes of the key
    /// in uppercase hex, 869.618 MHz / 62.5 kHz / SF8 / CR 5, 22 dBm.
    pub fn defaults(pub_key: &[u8; PUB_KEY_LEN]) -> Self {
        let mut name = [0u8; 32];
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for (i, b) in pub_key.iter().take(4).enumerate() {
            if let (Some(hi), Some(lo)) = (name.get_mut(2 * i), HEX.get(usize::from(b >> 4))) {
                *hi = *lo;
            }
            if let (Some(slot), Some(lo)) = (name.get_mut(2 * i + 1), HEX.get(usize::from(b & 0xF)))
            {
                *slot = *lo;
            }
        }
        Self {
            name,
            lat: 0,
            lon: 0,
            radio: RadioParams {
                freq_khz: 869_618,
                bw_hz: 62_500,
                sf: 8,
                cr: 5,
                tx_power_dbm: 22,
            },
            airtime_factor_milli: 1000,
            tx_delay_milli: 500,
            direct_tx_delay_milli: 200,
            rx_delay_base_milli: 0,
            multi_acks: 0,
            manual_add_contacts: 0,
            autoadd_config: 0,
            autoadd_max_hops: 0,
            telemetry_base: 0,
            telemetry_loc: 0,
            telemetry_env: 0,
            advert_loc_policy: 0,
            ble_pin: 0,
            repeat: false,
            path_hash_mode: 0,
            default_scope_name: [0; 31],
            default_scope_key: [0; 16],
            interference_threshold: 0,
            cad_enabled: false,
            gps_enabled: false,
            buzzer_quiet: false,
        }
    }

    pub fn path_hash_size(&self) -> u8 {
        self.path_hash_mode.min(2) + 1
    }
}

/// Bounded text (message, name…).
pub type Text = heapless::Vec<u8, { FRAME_MAX }>;

/// What the core reports to the companion layer and the UI.
#[derive(Debug, Clone)]
pub enum Event {
    /// Every received frame (`LOG_RX_DATA` notification).
    RxLog {
        snr4: i8,
        rssi: i8,
        raw: Text,
    },
    /// Message from a contact. `path_len`: the frame's own if it arrived by flood, 0xFF if
    /// direct. `author`: author prefix for a signed message (type 2).
    ContactMsg {
        key: [u8; PUB_KEY_LEN],
        snr4: i8,
        path_len: u8,
        txt_type: u8,
        ts: u32,
        author: Option<[u8; 4]>,
        text: Text,
    },
    ChannelMsg {
        idx: u8,
        snr4: i8,
        path_len: u8,
        txt_type: u8,
        ts: u32,
        text: Text,
    },
    ChannelData {
        idx: u8,
        snr4: i8,
        path_len: u8,
        data_type: u16,
        data: Text,
    },
    /// Advert from a known or just-added contact (`PUSH_CODE_ADVERT`). `raw`: the frame as we
    /// keep it (FLOOD route, no transport codes, `BaseChatMesh.cpp:141`).
    Advert {
        key: [u8; PUB_KEY_LEN],
        raw: Text,
    },
    /// Advert from a node we have not added (`PUSH_CODE_NEW_ADVERT`).
    NewAdvert {
        contact: crate::contacts::Contact,
    },
    ContactOverwritten {
        key: [u8; PUB_KEY_LEN],
    },
    ContactsFull,
    /// Advert heard (for `GET_ADVERT_PATH` and the Network screen).
    AdvertHeard {
        key: [u8; PUB_KEY_LEN],
        kind: u8,
        path_len: u8,
        path: heapless::Vec<u8, PATH_MAX>,
        snr4: i8,
    },
    PathUpdated {
        key: [u8; PUB_KEY_LEN],
    },
    SendConfirmed {
        ack: [u8; 4],
        rtt_ms: u32,
    },
    /// Response from a contact (login, status, telemetry, binary…); `data` starts with the tag.
    Response {
        key: [u8; PUB_KEY_LEN],
        data: Text,
    },
    /// Request from a contact (REQ type), to be handled by the companion layer.
    Request {
        key: [u8; PUB_KEY_LEN],
        ts: u32,
        data: Text,
        flood: bool,
        path_len: u8,
        path: heapless::Vec<u8, PATH_MAX>,
    },
    RawData {
        snr4: i8,
        rssi: i8,
        data: Text,
    },
    ControlData {
        snr4: i8,
        rssi: i8,
        path_len: u8,
        data: Text,
    },
    TraceData {
        path_len: u8,
        flags: u8,
        tag: u32,
        auth: u32,
        hashes: Text,
        snrs: heapless::Vec<u8, PATH_MAX>,
        final_snr4: i8,
    },
    /// Response to a path discovery (`PUSH_CODE_PATH_DISCOVERY_RESPONSE`): the path is not
    /// stored and no reciprocal path is sent (`MyMesh.cpp:779-808`).
    PathDiscovery {
        key: [u8; PUB_KEY_LEN],
        out_len: u8,
        out_path: heapless::Vec<u8, PATH_MAX>,
        in_len: u8,
        in_path: heapless::Vec<u8, PATH_MAX>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExpectedAck {
    pub ack: u32,
    pub sent: Instant,
    pub key: [u8; PUB_KEY_LEN],
}

/// Result of sending a message (`SENT` response).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub flood: bool,
    /// Expected ack CRC (plain text) or tag.
    pub ack: u32,
    pub timeout_ms: u32,
}

/// Send failure, with the C++ error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// ERR 2: unknown contact or channel.
    NotFound,
    /// ERR 3: too long, queue full, build failure.
    Failed,
    /// ERR 1: unsupported type.
    Unsupported,
}

pub struct Node {
    pub id: LocalIdentity,
    pub prefs: Prefs,
    pub rtc: Rtc,
    pub disp: Dispatcher,
    pub(crate) seen: Seen,
    pub contacts: Contacts,
    pub channels: Channels,
    pub(crate) acks: [Option<ExpectedAck>; ACK_TABLE],
    pub(crate) next_ack: usize,
    pub(crate) events: heapless::Deque<Event, 32>,
    /// In-RAM scope (`SET_FLOOD_SCOPE_KEY`) and request to send unscoped.
    pub send_scope: [u8; 16],
    pub send_unscoped: bool,
    pub last_snr4: i8,
    pub last_rssi: i16,
    pub boot: Instant,
    /// Tag of a path discovery in progress (`pending_discovery`).
    pub pending_discovery: u32,
}

impl core::fmt::Debug for Node {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Node")
            .field("id", &self.id)
            .field("contacts", &self.contacts.list.len())
            .finish_non_exhaustive()
    }
}

impl Node {
    pub fn new(id: LocalIdentity, prefs: Prefs, epoch: u32, now: Instant) -> Self {
        let mut n = Self::empty();
        n.reset(id, prefs, epoch, now);
        n
    }

    /// Empty node, constructible at compile time: on the device it lives in a static (87 KB,
    /// more than the stack) and [`Node::reset`] initializes it in place.
    pub const fn empty() -> Self {
        Self {
            id: LocalIdentity::PLACEHOLDER,
            prefs: Prefs::ZERO,
            rtc: Rtc::new(0, Instant::ZERO),
            disp: Dispatcher::new(Instant::ZERO, 1000),
            seen: Seen::new(),
            contacts: Contacts::new(),
            channels: Channels::new(),
            acks: [None; ACK_TABLE],
            next_ack: 0,
            events: heapless::Deque::new(),
            send_scope: [0; 16],
            send_unscoped: false,
            last_snr4: 0,
            last_rssi: 0,
            boot: Instant::ZERO,
            pending_discovery: 0,
        }
    }

    /// (Re)initializes the node in place: identity, settings, time; tables cleared.
    pub fn reset(&mut self, id: LocalIdentity, prefs: Prefs, epoch: u32, now: Instant) {
        let af = prefs.airtime_factor_milli;
        self.id = id;
        self.prefs = prefs;
        self.rtc = Rtc::new(epoch, now);
        self.disp = Dispatcher::new(now, af);
        self.seen = Seen::new();
        self.contacts.list.clear();
        self.contacts.anon = [const { None }; crate::contacts::ANON_SLOTS];
        self.channels = Channels::new();
        self.acks = [None; ACK_TABLE];
        self.next_ack = 0;
        self.events.clear();
        self.send_scope = [0; 16];
        self.send_unscoped = false;
        self.last_snr4 = 0;
        self.last_rssi = 0;
        self.boot = now;
        self.pending_discovery = 0;
    }

    pub(crate) fn emit(&mut self, e: Event) {
        if self.events.is_full() {
            let _ = self.events.pop_front();
        }
        let _ = self.events.push_back(e);
    }

    /// Next event for the companion layer or the UI.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Next frame to transmit now. `busy`: the radio is receiving (listen before talk).
    pub fn poll_tx(&mut self, now: Instant, busy: bool, rng: &mut impl Rng) -> Option<TxFrame> {
        let radio = self.prefs.radio;
        self.disp
            .poll_tx(now, busy, &radio, self.prefs.airtime_factor_milli, rng)
    }

    pub fn tx_done(&mut self, now: Instant) {
        self.disp.tx_done(now, self.prefs.airtime_factor_milli);
    }

    pub fn next_wakeup(&self) -> Option<Instant> {
        self.disp.next_wakeup()
    }

    /// Current Unix time.
    pub fn time(&self, now: Instant) -> u32 {
        self.rtc.now(now)
    }

    // ----------------------------------------------------------------------------- transmit

    /// `sendFlood`: empty path with hashes of the configured size, priority 1 (PATH 2, ADVERT 3).
    pub(crate) fn send_flood(
        &mut self,
        mut pkt: PacketBuf,
        transport: Option<[u16; 2]>,
        delay_ms: u64,
        now: Instant,
    ) {
        if pkt.header.ptype == PayloadType::TRACE {
            return;
        }
        pkt.header.route = if transport.is_some() {
            Route::TransportFlood
        } else {
            Route::Flood
        };
        pkt.transport = transport;
        let size = self.prefs.path_hash_size();
        if let Ok(pl) = PathLen::new(size, 0) {
            let _ = pkt.set_path(pl, &[]);
        }
        self.seen.mark(&pkt.hash());
        let pri = match pkt.header.ptype {
            PayloadType::PATH => 2,
            PayloadType::ADVERT => 3,
            _ => 1,
        };
        self.disp
            .queue(pkt, pri, now + Duration::from_millis(delay_ms));
    }

    /// The companion's `sendFloodScoped`: unscoped if requested, else the in-RAM scope, else the
    /// default scope, else unscoped (`MyMesh.cpp:491-525`).
    pub(crate) fn send_flood_scoped(&mut self, pkt: PacketBuf, delay_ms: u64, now: Instant) {
        let key = if self.send_unscoped {
            None
        } else if self.send_scope != [0; 16] {
            Some(self.send_scope)
        } else if self.prefs.default_scope_key != [0; 16] {
            Some(self.prefs.default_scope_key)
        } else {
            None
        };
        let transport = key.map(|k| {
            [
                mc_air::crypto::transport_code(&k, pkt.header.ptype.value(), pkt.payload()),
                0,
            ]
        });
        self.send_flood(pkt, transport, delay_ms, now);
    }

    /// `sendDirect`: the given path, priority 0 (PATH 1).
    pub(crate) fn send_direct(
        &mut self,
        mut pkt: PacketBuf,
        path_len: u8,
        path: &[u8],
        delay_ms: u64,
        now: Instant,
    ) {
        pkt.header.route = Route::Direct;
        pkt.transport = None;
        let pri = if pkt.header.ptype == PayloadType::TRACE {
            // A trace's path goes at the end of the payload; path_len = 0.
            let mut payload = [0u8; mc_air::packet::PAYLOAD_MAX];
            let cur = pkt.payload();
            let total = cur.len() + path.len();
            if let Some(dst) = payload.get_mut(..total) {
                let (a, b) = dst.split_at_mut(cur.len());
                a.copy_from_slice(cur);
                b.copy_from_slice(path);
            }
            if pkt
                .set_payload(payload.get(..total).unwrap_or(&[]))
                .is_err()
            {
                return;
            }
            let _ = pkt.set_path(PathLen::EMPTY, &[]);
            5
        } else {
            match PathLen::from_byte(path_len) {
                Ok(pl)
                    if pkt
                        .set_path(pl, path.get(..pl.byte_len()).unwrap_or(&[]))
                        .is_ok() => {}
                _ => return,
            }
            if pkt.header.ptype == PayloadType::PATH {
                1
            } else {
                0
            }
        };
        self.seen.mark(&pkt.hash());
        self.disp
            .queue(pkt, pri, now + Duration::from_millis(delay_ms));
    }

    /// `sendZeroHop`: direct route, empty path, priority 0.
    pub(crate) fn send_zero_hop(
        &mut self,
        mut pkt: PacketBuf,
        transport: Option<[u16; 2]>,
        delay_ms: u64,
        now: Instant,
    ) {
        pkt.header.route = if transport.is_some() {
            Route::TransportDirect
        } else {
            Route::Direct
        };
        pkt.transport = transport;
        let _ = pkt.set_path(PathLen::EMPTY, &[]);
        self.seen.mark(&pkt.hash());
        self.disp
            .queue(pkt, 0, now + Duration::from_millis(delay_ms));
    }

    /// Flood retransmit delay (`MyMesh::getRetransmitDelay`): `rand[0, 5t]`,
    /// `t = airtime(path + payload + 2) × tx_delay`.
    pub(crate) fn retransmit_delay(
        &self,
        pkt: &PacketBuf,
        factor_milli: u32,
        rng: &mut impl Rng,
    ) -> u64 {
        let t = self
            .prefs
            .radio
            .airtime_ms(pkt.path().len() + pkt.payload().len() + 2)
            * u64::from(factor_milli)
            / 1000;
        u64::from(next_int(
            rng,
            0,
            u32::try_from(5 * t + 1).unwrap_or(u32::MAX),
        ))
    }
}
