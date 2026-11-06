//! Device responses (codes 0..29) and notifications (0x80..0x90) (`docs/spec/companion.md` §3 to
//! §5). Strict, canonical decoding; a code outside v1.18 becomes [`Response::Other`].

use core::fmt;
use mc_core::{Hex, Overflow, Reader, Writer};

use crate::codec::{DecodeError, FRAME_MAX, Text, end, fixed_str, i8_of, read_path, resp};

/// Device contact (CONTACT, NEW_ADVERT), §3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contact<'a> {
    pub key: [u8; 32],
    pub kind: u8,
    pub flags: u8,
    pub path_len: u8,
    pub path: [u8; 64],
    pub name: [u8; 32],
    pub last_advert: u32,
    pub lat: i32,
    pub lon: i32,
    pub lastmod: u32,
    /// Fields added by a newer firmware, kept as is.
    pub ext: &'a [u8],
}

impl<'a> Contact<'a> {
    fn read(r: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            key: *r.array()?,
            kind: r.u8()?,
            flags: r.u8()?,
            path_len: r.u8()?,
            path: *r.array()?,
            name: *r.array()?,
            last_advert: r.u32_le()?,
            lat: r.i32_le()?,
            lon: r.i32_le()?,
            lastmod: r.u32_le()?,
            ext: r.rest(),
        })
    }

    fn write(&self, w: &mut Writer<'_>) -> Result<(), Overflow> {
        w.put(&self.key)?;
        w.put_u8(self.kind)?;
        w.put_u8(self.flags)?;
        w.put_u8(self.path_len)?;
        w.put(&self.path)?;
        w.put(&self.name)?;
        w.put_u32_le(self.last_advert)?;
        w.put_i32_le(self.lat)?;
        w.put_i32_le(self.lon)?;
        w.put_u32_le(self.lastmod)?;
        w.put(self.ext)
    }
}

/// DEVICE_INFO, §3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInfo<'a> {
    pub fw_ver: u8,
    pub max_contacts_half: u8,
    pub max_channels: u8,
    pub ble_pin: u32,
    pub build_date: [u8; 12],
    pub manufacturer: [u8; 40],
    pub version: [u8; 20],
    pub repeat: u8,
    pub path_hash_mode: u8,
    pub ext: &'a [u8],
}

/// SELF_INFO, §3.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfInfo<'a> {
    pub adv_type: u8,
    pub tx_power: u8,
    pub max_tx_power: u8,
    pub key: [u8; 32],
    pub lat: i32,
    pub lon: i32,
    pub multi_acks: u8,
    pub advert_loc_policy: u8,
    pub telemetry: u8,
    pub manual_add: u8,
    pub freq_khz: u32,
    pub bw_hz: u32,
    pub sf: u8,
    pub cr: u8,
    pub node_name: &'a [u8],
}

/// Tail of LOGIN_SUCCESS in the new format (14 bytes, M:700-713).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginServer {
    /// Server time (the "tag" of its response).
    pub server_time: u32,
    /// Permissions ACL (v7+).
    pub acl: u8,
    /// The server's `FIRMWARE_VER_LEVEL`.
    pub fw_level: u8,
}

/// STATS (24) body, depending on the requested type (M:1914-1964).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stats {
    /// Type 0.
    Core {
        battery_mv: u16,
        uptime_s: u32,
        err_flags: u16,
        queue_len: u8,
    },
    /// Type 1.
    Radio {
        noise_floor: i16,
        last_rssi: i8,
        last_snr4: i8,
        tx_air_s: u32,
        rx_air_s: u32,
    },
    /// Type 2.
    Packets {
        recv: u32,
        sent: u32,
        sent_flood: u32,
        sent_direct: u32,
        recv_flood: u32,
        recv_direct: u32,
        recv_errors: u32,
    },
}

/// A device response or notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response<'a> {
    /// Response 0.
    Ok,
    /// Response 1, with the error code.
    Err(u8),
    /// Response 2: total number of contacts (unfiltered).
    ContactsStart { total: u32 },
    /// Response 3.
    Contact(Contact<'a>),
    /// Response 4: largest `lastmod` sent.
    EndOfContacts { lastmod: u32 },
    /// Response 5.
    SelfInfo(SelfInfo<'a>),
    /// Response 6. `ack`: expected ack, or tag of a request.
    Sent {
        flood: bool,
        ack: [u8; 4],
        timeout_ms: u32,
    },
    /// Response 7: direct message, old format (`app_target_ver` < 3), like 16 without `[1..3]`.
    /// For `txt_type` 2, `text` starts with the 4 bytes of the author's prefix.
    ContactMsgLegacy {
        prefix: [u8; 6],
        path_len: u8,
        txt_type: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    /// Response 8: channel message, old format: like 17 without `[1..3]`.
    ChannelMsgLegacy {
        idx: u8,
        path_len: u8,
        txt_type: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    /// Response 9.
    CurrTime { unix_s: u32 },
    /// Response 10.
    NoMoreMessages,
    /// Response 11: raw advert packet (§3.3).
    ExportContact { raw: &'a [u8] },
    /// Response 12.
    BattAndStorage {
        battery_mv: u16,
        used_kb: u32,
        total_kb: u32,
    },
    /// Response 13.
    DeviceInfo(DeviceInfo<'a>),
    /// Response 14.
    PrivateKey { key: [u8; 64] },
    /// Response 15: feature disabled at compile time (key export or import).
    Disabled,
    /// Response 16: direct message (V3). For `txt_type` 2, `text` starts with the 4 bytes of the
    /// author's prefix.
    ContactMsg {
        snr4: i8,
        reserved: [u8; 2],
        prefix: [u8; 6],
        path_len: u8,
        txt_type: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    /// Response 17: channel message (V3), `"Name: text"`.
    ChannelMsg {
        snr4: i8,
        reserved: [u8; 2],
        idx: u8,
        path_len: u8,
        txt_type: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    /// Response 18.
    ChannelInfo {
        idx: u8,
        name: [u8; 32],
        secret: [u8; 16],
    },
    /// Response 19: maximum size of the data to sign.
    SignStart { reserved: u8, max_len: u32 },
    /// Response 20.
    Signature { signature: [u8; 64] },
    /// Response 21: `"name:value,name:value"`.
    CustomVars { vars: &'a [u8] },
    /// Response 22: reception time (device clock) and path of the advert.
    AdvertPath {
        recv_ts: u32,
        path_len: u8,
        path: &'a [u8],
    },
    /// Response 23: values × 1000.
    TuningParams { rx_delay: u32, airtime_factor: u32 },
    /// Response 24.
    Stats(Stats),
    /// Response 25.
    AutoaddConfig { config: u8, max_hops: u8 },
    /// Response 26: `(low u32, high u32)` pairs in kHz, see [`freq_ranges`].
    AllowedRepeatFreq { ranges: &'a [u8] },
    /// Response 27.
    ChannelData {
        snr4: i8,
        reserved: [u8; 2],
        idx: u8,
        path_len: u8,
        data_type: u16,
        data: &'a [u8],
    },
    /// Response 28. `None`: no default scope.
    DefaultFloodScope { scope: Option<([u8; 31], [u8; 16])> },
    /// Response 29.
    CliReply { text: &'a [u8] },
    /// Notification 0x80.
    Advert { key: [u8; 32] },
    /// Notification 0x81.
    PathUpdated { key: [u8; 32] },
    /// Notification 0x82.
    SendConfirmed { ack: [u8; 4], rtt_ms: u32 },
    /// Notification 0x83.
    MsgWaiting,
    /// Notification 0x84. `reserved` is 0xFF.
    RawData {
        snr4: i8,
        rssi: i8,
        reserved: u8,
        payload: &'a [u8],
    },
    /// Notification 0x85. Old format (8 bytes): `server` absent and `permissions` set to 0.
    LoginSuccess {
        permissions: u8,
        prefix: [u8; 6],
        server: Option<LoginServer>,
    },
    /// Notification 0x86.
    LoginFail { reserved: u8, prefix: [u8; 6] },
    /// Notification 0x87.
    StatusResponse {
        reserved: u8,
        prefix: [u8; 6],
        data: &'a [u8],
    },
    /// Notification 0x88: every packet received by the radio.
    LogRxData { snr4: i8, rssi: i8, raw: &'a [u8] },
    /// Notification 0x89. `hashes` (`path_len` bytes), then `path_len >> (flags & 3)` SNR × 4.
    TraceData {
        reserved: u8,
        flags: u8,
        tag: u32,
        auth: u32,
        hashes: &'a [u8],
        snrs: &'a [u8],
        final_snr4: i8,
    },
    /// Notification 0x8A.
    NewAdvert(Contact<'a>),
    /// Notification 0x8B: CayenneLPP data.
    TelemetryResponse {
        reserved: u8,
        prefix: [u8; 6],
        lpp: &'a [u8],
    },
    /// Notification 0x8C.
    BinaryResponse {
        reserved: u8,
        tag: [u8; 4],
        data: &'a [u8],
    },
    /// Notification 0x8D.
    PathDiscoveryResponse {
        reserved: u8,
        prefix: [u8; 6],
        out_path_len: u8,
        out_path: &'a [u8],
        in_path_len: u8,
        in_path: &'a [u8],
    },
    /// Notification 0x8E.
    ControlData {
        snr4: i8,
        rssi: i8,
        path_len: u8,
        payload: &'a [u8],
    },
    /// Notification 0x8F.
    ContactDeleted { key: [u8; 32] },
    /// Notification 0x90.
    ContactsFull,
    /// Code outside v1.18: ignored by the session.
    Other { code: u8, body: &'a [u8] },
}

/// The `(low, high)` pairs in kHz of ALLOWED_REPEAT_FREQ. A remainder shorter than 8 bytes is
/// ignored (strict decoding rejects it).
pub fn freq_ranges(ranges: &[u8]) -> impl Iterator<Item = (u32, u32)> + '_ {
    ranges.as_chunks::<8>().0.iter().map(|c| {
        let mut r = Reader::new(c);
        (r.u32_le().unwrap_or(0), r.u32_le().unwrap_or(0))
    })
}

fn put_i8(w: &mut Writer<'_>, v: i8) -> Result<(), Overflow> {
    w.put(&v.to_le_bytes())
}

impl<'a> Response<'a> {
    /// True for a notification (code ≥ 0x80), which answers no command.
    pub fn is_push(frame: &[u8]) -> bool {
        frame.first().is_some_and(|c| *c >= 0x80)
    }

    /// Strict, canonical decoding.
    pub fn decode(frame: &'a [u8]) -> Result<Self, DecodeError> {
        if frame.len() > FRAME_MAX {
            return Err(DecodeError::Length);
        }
        let mut r = Reader::new(frame);
        let code = r.u8()?;
        let v = match code {
            resp::OK => Response::Ok,
            resp::ERR => Response::Err(r.u8()?),
            resp::CONTACTS_START => Response::ContactsStart { total: r.u32_le()? },
            resp::CONTACT => Response::Contact(Contact::read(&mut r)?),
            resp::END_OF_CONTACTS => Response::EndOfContacts {
                lastmod: r.u32_le()?,
            },
            resp::SELF_INFO => Response::SelfInfo(SelfInfo {
                adv_type: r.u8()?,
                tx_power: r.u8()?,
                max_tx_power: r.u8()?,
                key: *r.array()?,
                lat: r.i32_le()?,
                lon: r.i32_le()?,
                multi_acks: r.u8()?,
                advert_loc_policy: r.u8()?,
                telemetry: r.u8()?,
                manual_add: r.u8()?,
                freq_khz: r.u32_le()?,
                bw_hz: r.u32_le()?,
                sf: r.u8()?,
                cr: r.u8()?,
                node_name: r.rest(),
            }),
            resp::SENT => Response::Sent {
                flood: match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::Value),
                },
                ack: *r.array()?,
                timeout_ms: r.u32_le()?,
            },
            resp::CONTACT_MSG_RECV => Response::ContactMsgLegacy {
                prefix: *r.array()?,
                path_len: r.u8()?,
                txt_type: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            resp::CHANNEL_MSG_RECV => Response::ChannelMsgLegacy {
                idx: r.u8()?,
                path_len: r.u8()?,
                txt_type: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            resp::CURR_TIME => Response::CurrTime {
                unix_s: r.u32_le()?,
            },
            resp::NO_MORE_MESSAGES => Response::NoMoreMessages,
            resp::EXPORT_CONTACT => Response::ExportContact { raw: r.rest() },
            resp::BATT_AND_STORAGE => Response::BattAndStorage {
                battery_mv: r.u16_le()?,
                used_kb: r.u32_le()?,
                total_kb: r.u32_le()?,
            },
            resp::DEVICE_INFO => Response::DeviceInfo(DeviceInfo {
                fw_ver: r.u8()?,
                max_contacts_half: r.u8()?,
                max_channels: r.u8()?,
                ble_pin: r.u32_le()?,
                build_date: *r.array()?,
                manufacturer: *r.array()?,
                version: *r.array()?,
                repeat: r.u8()?,
                path_hash_mode: r.u8()?,
                ext: r.rest(),
            }),
            resp::PRIVATE_KEY => Response::PrivateKey { key: *r.array()? },
            resp::DISABLED => Response::Disabled,
            resp::CONTACT_MSG_RECV_V3 => Response::ContactMsg {
                snr4: i8_of(r.u8()?),
                reserved: *r.array()?,
                prefix: *r.array()?,
                path_len: r.u8()?,
                txt_type: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            resp::CHANNEL_MSG_RECV_V3 => Response::ChannelMsg {
                snr4: i8_of(r.u8()?),
                reserved: *r.array()?,
                idx: r.u8()?,
                path_len: r.u8()?,
                txt_type: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            resp::CHANNEL_INFO => Response::ChannelInfo {
                idx: r.u8()?,
                name: *r.array()?,
                secret: *r.array()?,
            },
            resp::SIGN_START => Response::SignStart {
                reserved: r.u8()?,
                max_len: r.u32_le()?,
            },
            resp::SIGNATURE => Response::Signature {
                signature: *r.array()?,
            },
            resp::CUSTOM_VARS => Response::CustomVars { vars: r.rest() },
            resp::ADVERT_PATH => {
                let recv_ts = r.u32_le()?;
                let (path_len, path) = read_path(&mut r)?;
                Response::AdvertPath {
                    recv_ts,
                    path_len,
                    path,
                }
            }
            resp::TUNING_PARAMS => Response::TuningParams {
                rx_delay: r.u32_le()?,
                airtime_factor: r.u32_le()?,
            },
            resp::STATS => Response::Stats(match r.u8()? {
                0 => Stats::Core {
                    battery_mv: r.u16_le()?,
                    uptime_s: r.u32_le()?,
                    err_flags: r.u16_le()?,
                    queue_len: r.u8()?,
                },
                1 => Stats::Radio {
                    noise_floor: i16::from_le_bytes(*r.array()?),
                    last_rssi: i8_of(r.u8()?),
                    last_snr4: i8_of(r.u8()?),
                    tx_air_s: r.u32_le()?,
                    rx_air_s: r.u32_le()?,
                },
                2 => Stats::Packets {
                    recv: r.u32_le()?,
                    sent: r.u32_le()?,
                    sent_flood: r.u32_le()?,
                    sent_direct: r.u32_le()?,
                    recv_flood: r.u32_le()?,
                    recv_direct: r.u32_le()?,
                    recv_errors: r.u32_le()?,
                },
                _ => return Err(DecodeError::Value),
            }),
            resp::AUTOADD_CONFIG => Response::AutoaddConfig {
                config: r.u8()?,
                max_hops: r.u8()?,
            },
            resp::ALLOWED_REPEAT_FREQ => {
                let ranges = r.rest();
                if !ranges.len().is_multiple_of(8) {
                    return Err(DecodeError::Length);
                }
                Response::AllowedRepeatFreq { ranges }
            }
            resp::CHANNEL_DATA_RECV => {
                let snr4 = i8_of(r.u8()?);
                let reserved = *r.array()?;
                let idx = r.u8()?;
                let path_len = r.u8()?;
                let data_type = r.u16_le()?;
                let data = r.len8()?;
                Response::ChannelData {
                    snr4,
                    reserved,
                    idx,
                    path_len,
                    data_type,
                    data,
                }
            }
            resp::DEFAULT_FLOOD_SCOPE => Response::DefaultFloodScope {
                scope: if r.is_empty() {
                    None
                } else {
                    Some((*r.array()?, *r.array()?))
                },
            },
            resp::CLI_REPLY => Response::CliReply { text: r.rest() },
            resp::ADVERT => Response::Advert { key: *r.array()? },
            resp::PATH_UPDATED => Response::PathUpdated { key: *r.array()? },
            resp::SEND_CONFIRMED => Response::SendConfirmed {
                ack: *r.array()?,
                rtt_ms: r.u32_le()?,
            },
            resp::MSG_WAITING => Response::MsgWaiting,
            resp::RAW_DATA => Response::RawData {
                snr4: i8_of(r.u8()?),
                rssi: i8_of(r.u8()?),
                reserved: r.u8()?,
                payload: r.rest(),
            },
            resp::LOGIN_SUCCESS => {
                let permissions = r.u8()?;
                let prefix = *r.array()?;
                let server = if r.is_empty() {
                    None
                } else {
                    Some(LoginServer {
                        server_time: r.u32_le()?,
                        acl: r.u8()?,
                        fw_level: r.u8()?,
                    })
                };
                Response::LoginSuccess {
                    permissions,
                    prefix,
                    server,
                }
            }
            resp::LOGIN_FAIL => Response::LoginFail {
                reserved: r.u8()?,
                prefix: *r.array()?,
            },
            resp::STATUS_RESPONSE => Response::StatusResponse {
                reserved: r.u8()?,
                prefix: *r.array()?,
                data: r.rest(),
            },
            resp::LOG_RX_DATA => Response::LogRxData {
                snr4: i8_of(r.u8()?),
                rssi: i8_of(r.u8()?),
                raw: r.rest(),
            },
            resp::TRACE_DATA => {
                let reserved = r.u8()?;
                let path_len = r.u8()?;
                let flags = r.u8()?;
                let tag = r.u32_le()?;
                let auth = r.u32_le()?;
                let hashes = r.take(usize::from(path_len))?;
                let snrs = r.take(usize::from(path_len >> (flags & 3)))?;
                Response::TraceData {
                    reserved,
                    flags,
                    tag,
                    auth,
                    hashes,
                    snrs,
                    final_snr4: i8_of(r.u8()?),
                }
            }
            resp::NEW_ADVERT => Response::NewAdvert(Contact::read(&mut r)?),
            resp::TELEMETRY_RESPONSE => Response::TelemetryResponse {
                reserved: r.u8()?,
                prefix: *r.array()?,
                lpp: r.rest(),
            },
            resp::BINARY_RESPONSE => Response::BinaryResponse {
                reserved: r.u8()?,
                tag: *r.array()?,
                data: r.rest(),
            },
            resp::PATH_DISCOVERY_RESPONSE => {
                let reserved = r.u8()?;
                let prefix = *r.array()?;
                let (out_path_len, out_path) = read_path(&mut r)?;
                let (in_path_len, in_path) = read_path(&mut r)?;
                Response::PathDiscoveryResponse {
                    reserved,
                    prefix,
                    out_path_len,
                    out_path,
                    in_path_len,
                    in_path,
                }
            }
            resp::CONTROL_DATA => Response::ControlData {
                snr4: i8_of(r.u8()?),
                rssi: i8_of(r.u8()?),
                path_len: r.u8()?,
                payload: r.rest(),
            },
            resp::CONTACT_DELETED => Response::ContactDeleted { key: *r.array()? },
            resp::CONTACTS_FULL => Response::ContactsFull,
            code => Response::Other {
                code,
                body: r.rest(),
            },
        };
        end(&r)?;
        Ok(v)
    }

    /// Writes the frame into `out`. Returns its length.
    ///
    /// Fails if `out` is too small, or if the value has no canonical form (strict decoding of
    /// the written frame would reject it or read something else: [`Response::Other`] with a
    /// known code, wrong number of SNRs in a TRACE, frame longer than 176 bytes…).
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let n = self.write(out)?;
        match out.get(..n).map(Response::decode) {
            Some(Ok(back)) if back == *self => Ok(n),
            _ => Err(Overflow),
        }
    }

    fn write(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let mut w = Writer::new(out);
        w.put_u8(self.code())?;
        match self {
            Response::Err(c) => w.put_u8(*c)?,
            Response::ContactsStart { total } => w.put_u32_le(*total)?,
            Response::Contact(c) | Response::NewAdvert(c) => c.write(&mut w)?,
            Response::EndOfContacts { lastmod } => w.put_u32_le(*lastmod)?,
            Response::SelfInfo(s) => {
                w.put_u8(s.adv_type)?;
                w.put_u8(s.tx_power)?;
                w.put_u8(s.max_tx_power)?;
                w.put(&s.key)?;
                w.put_i32_le(s.lat)?;
                w.put_i32_le(s.lon)?;
                w.put_u8(s.multi_acks)?;
                w.put_u8(s.advert_loc_policy)?;
                w.put_u8(s.telemetry)?;
                w.put_u8(s.manual_add)?;
                w.put_u32_le(s.freq_khz)?;
                w.put_u32_le(s.bw_hz)?;
                w.put_u8(s.sf)?;
                w.put_u8(s.cr)?;
                w.put(s.node_name)?;
            }
            Response::Sent {
                flood,
                ack,
                timeout_ms,
            } => {
                w.put_u8(u8::from(*flood))?;
                w.put(ack)?;
                w.put_u32_le(*timeout_ms)?;
            }
            Response::ContactMsgLegacy {
                prefix,
                path_len,
                txt_type,
                timestamp,
                text,
            } => {
                w.put(prefix)?;
                w.put_u8(*path_len)?;
                w.put_u8(*txt_type)?;
                w.put_u32_le(*timestamp)?;
                w.put(text)?;
            }
            Response::ChannelMsgLegacy {
                idx,
                path_len,
                txt_type,
                timestamp,
                text,
            } => {
                w.put_u8(*idx)?;
                w.put_u8(*path_len)?;
                w.put_u8(*txt_type)?;
                w.put_u32_le(*timestamp)?;
                w.put(text)?;
            }
            Response::CurrTime { unix_s } => w.put_u32_le(*unix_s)?,
            Response::ExportContact { raw } => w.put(raw)?,
            Response::BattAndStorage {
                battery_mv,
                used_kb,
                total_kb,
            } => {
                w.put_u16_le(*battery_mv)?;
                w.put_u32_le(*used_kb)?;
                w.put_u32_le(*total_kb)?;
            }
            Response::DeviceInfo(d) => {
                w.put_u8(d.fw_ver)?;
                w.put_u8(d.max_contacts_half)?;
                w.put_u8(d.max_channels)?;
                w.put_u32_le(d.ble_pin)?;
                w.put(&d.build_date)?;
                w.put(&d.manufacturer)?;
                w.put(&d.version)?;
                w.put_u8(d.repeat)?;
                w.put_u8(d.path_hash_mode)?;
                w.put(d.ext)?;
            }
            Response::PrivateKey { key } => w.put(key)?,
            Response::ContactMsg {
                snr4,
                reserved,
                prefix,
                path_len,
                txt_type,
                timestamp,
                text,
            } => {
                put_i8(&mut w, *snr4)?;
                w.put(reserved)?;
                w.put(prefix)?;
                w.put_u8(*path_len)?;
                w.put_u8(*txt_type)?;
                w.put_u32_le(*timestamp)?;
                w.put(text)?;
            }
            Response::ChannelMsg {
                snr4,
                reserved,
                idx,
                path_len,
                txt_type,
                timestamp,
                text,
            } => {
                put_i8(&mut w, *snr4)?;
                w.put(reserved)?;
                w.put_u8(*idx)?;
                w.put_u8(*path_len)?;
                w.put_u8(*txt_type)?;
                w.put_u32_le(*timestamp)?;
                w.put(text)?;
            }
            Response::ChannelInfo { idx, name, secret } => {
                w.put_u8(*idx)?;
                w.put(name)?;
                w.put(secret)?;
            }
            Response::SignStart { reserved, max_len } => {
                w.put_u8(*reserved)?;
                w.put_u32_le(*max_len)?;
            }
            Response::Signature { signature } => w.put(signature)?,
            Response::CustomVars { vars } => w.put(vars)?,
            Response::AdvertPath {
                recv_ts,
                path_len,
                path,
            } => {
                w.put_u32_le(*recv_ts)?;
                w.put_u8(*path_len)?;
                w.put(path)?;
            }
            Response::TuningParams {
                rx_delay,
                airtime_factor,
            } => {
                w.put_u32_le(*rx_delay)?;
                w.put_u32_le(*airtime_factor)?;
            }
            Response::Stats(s) => match s {
                Stats::Core {
                    battery_mv,
                    uptime_s,
                    err_flags,
                    queue_len,
                } => {
                    w.put_u8(0)?;
                    w.put_u16_le(*battery_mv)?;
                    w.put_u32_le(*uptime_s)?;
                    w.put_u16_le(*err_flags)?;
                    w.put_u8(*queue_len)?;
                }
                Stats::Radio {
                    noise_floor,
                    last_rssi,
                    last_snr4,
                    tx_air_s,
                    rx_air_s,
                } => {
                    w.put_u8(1)?;
                    w.put(&noise_floor.to_le_bytes())?;
                    put_i8(&mut w, *last_rssi)?;
                    put_i8(&mut w, *last_snr4)?;
                    w.put_u32_le(*tx_air_s)?;
                    w.put_u32_le(*rx_air_s)?;
                }
                Stats::Packets {
                    recv,
                    sent,
                    sent_flood,
                    sent_direct,
                    recv_flood,
                    recv_direct,
                    recv_errors,
                } => {
                    w.put_u8(2)?;
                    for v in [
                        recv,
                        sent,
                        sent_flood,
                        sent_direct,
                        recv_flood,
                        recv_direct,
                        recv_errors,
                    ] {
                        w.put_u32_le(*v)?;
                    }
                }
            },
            Response::AutoaddConfig { config, max_hops } => {
                w.put_u8(*config)?;
                w.put_u8(*max_hops)?;
            }
            Response::AllowedRepeatFreq { ranges } => w.put(ranges)?,
            Response::ChannelData {
                snr4,
                reserved,
                idx,
                path_len,
                data_type,
                data,
            } => {
                put_i8(&mut w, *snr4)?;
                w.put(reserved)?;
                w.put_u8(*idx)?;
                w.put_u8(*path_len)?;
                w.put_u16_le(*data_type)?;
                w.put_len8(data)?;
            }
            Response::DefaultFloodScope { scope } => {
                if let Some((name, key)) = scope {
                    w.put(name)?;
                    w.put(key)?;
                }
            }
            Response::CliReply { text } => w.put(text)?,
            Response::Advert { key }
            | Response::PathUpdated { key }
            | Response::ContactDeleted { key } => w.put(key)?,
            Response::SendConfirmed { ack, rtt_ms } => {
                w.put(ack)?;
                w.put_u32_le(*rtt_ms)?;
            }
            Response::RawData {
                snr4,
                rssi,
                reserved,
                payload,
            } => {
                put_i8(&mut w, *snr4)?;
                put_i8(&mut w, *rssi)?;
                w.put_u8(*reserved)?;
                w.put(payload)?;
            }
            Response::LoginSuccess {
                permissions,
                prefix,
                server,
            } => {
                w.put_u8(*permissions)?;
                w.put(prefix)?;
                if let Some(s) = server {
                    w.put_u32_le(s.server_time)?;
                    w.put_u8(s.acl)?;
                    w.put_u8(s.fw_level)?;
                }
            }
            Response::LoginFail { reserved, prefix } => {
                w.put_u8(*reserved)?;
                w.put(prefix)?;
            }
            Response::StatusResponse {
                reserved,
                prefix,
                data,
            } => {
                w.put_u8(*reserved)?;
                w.put(prefix)?;
                w.put(data)?;
            }
            Response::LogRxData { snr4, rssi, raw } => {
                put_i8(&mut w, *snr4)?;
                put_i8(&mut w, *rssi)?;
                w.put(raw)?;
            }
            Response::TraceData {
                reserved,
                flags,
                tag,
                auth,
                hashes,
                snrs,
                final_snr4,
            } => {
                w.put_u8(*reserved)?;
                w.put_u8(u8::try_from(hashes.len()).map_err(|_| Overflow)?)?;
                w.put_u8(*flags)?;
                w.put_u32_le(*tag)?;
                w.put_u32_le(*auth)?;
                w.put(hashes)?;
                w.put(snrs)?;
                put_i8(&mut w, *final_snr4)?;
            }
            Response::TelemetryResponse {
                reserved,
                prefix,
                lpp,
            } => {
                w.put_u8(*reserved)?;
                w.put(prefix)?;
                w.put(lpp)?;
            }
            Response::BinaryResponse {
                reserved,
                tag,
                data,
            } => {
                w.put_u8(*reserved)?;
                w.put(tag)?;
                w.put(data)?;
            }
            Response::PathDiscoveryResponse {
                reserved,
                prefix,
                out_path_len,
                out_path,
                in_path_len,
                in_path,
            } => {
                w.put_u8(*reserved)?;
                w.put(prefix)?;
                w.put_u8(*out_path_len)?;
                w.put(out_path)?;
                w.put_u8(*in_path_len)?;
                w.put(in_path)?;
            }
            Response::ControlData {
                snr4,
                rssi,
                path_len,
                payload,
            } => {
                put_i8(&mut w, *snr4)?;
                put_i8(&mut w, *rssi)?;
                w.put_u8(*path_len)?;
                w.put(payload)?;
            }
            Response::Other { body, .. } => w.put(body)?,
            Response::Ok
            | Response::NoMoreMessages
            | Response::Disabled
            | Response::MsgWaiting
            | Response::ContactsFull => {}
        }
        Ok(w.position())
    }

    /// Response code.
    pub const fn code(&self) -> u8 {
        match self {
            Response::Ok => resp::OK,
            Response::Err(_) => resp::ERR,
            Response::ContactsStart { .. } => resp::CONTACTS_START,
            Response::Contact(_) => resp::CONTACT,
            Response::EndOfContacts { .. } => resp::END_OF_CONTACTS,
            Response::SelfInfo(_) => resp::SELF_INFO,
            Response::Sent { .. } => resp::SENT,
            Response::ContactMsgLegacy { .. } => resp::CONTACT_MSG_RECV,
            Response::ChannelMsgLegacy { .. } => resp::CHANNEL_MSG_RECV,
            Response::CurrTime { .. } => resp::CURR_TIME,
            Response::NoMoreMessages => resp::NO_MORE_MESSAGES,
            Response::ExportContact { .. } => resp::EXPORT_CONTACT,
            Response::BattAndStorage { .. } => resp::BATT_AND_STORAGE,
            Response::DeviceInfo(_) => resp::DEVICE_INFO,
            Response::PrivateKey { .. } => resp::PRIVATE_KEY,
            Response::Disabled => resp::DISABLED,
            Response::ContactMsg { .. } => resp::CONTACT_MSG_RECV_V3,
            Response::ChannelMsg { .. } => resp::CHANNEL_MSG_RECV_V3,
            Response::ChannelInfo { .. } => resp::CHANNEL_INFO,
            Response::SignStart { .. } => resp::SIGN_START,
            Response::Signature { .. } => resp::SIGNATURE,
            Response::CustomVars { .. } => resp::CUSTOM_VARS,
            Response::AdvertPath { .. } => resp::ADVERT_PATH,
            Response::TuningParams { .. } => resp::TUNING_PARAMS,
            Response::Stats(_) => resp::STATS,
            Response::AutoaddConfig { .. } => resp::AUTOADD_CONFIG,
            Response::AllowedRepeatFreq { .. } => resp::ALLOWED_REPEAT_FREQ,
            Response::ChannelData { .. } => resp::CHANNEL_DATA_RECV,
            Response::DefaultFloodScope { .. } => resp::DEFAULT_FLOOD_SCOPE,
            Response::CliReply { .. } => resp::CLI_REPLY,
            Response::Advert { .. } => resp::ADVERT,
            Response::PathUpdated { .. } => resp::PATH_UPDATED,
            Response::SendConfirmed { .. } => resp::SEND_CONFIRMED,
            Response::MsgWaiting => resp::MSG_WAITING,
            Response::RawData { .. } => resp::RAW_DATA,
            Response::LoginSuccess { .. } => resp::LOGIN_SUCCESS,
            Response::LoginFail { .. } => resp::LOGIN_FAIL,
            Response::StatusResponse { .. } => resp::STATUS_RESPONSE,
            Response::LogRxData { .. } => resp::LOG_RX_DATA,
            Response::TraceData { .. } => resp::TRACE_DATA,
            Response::NewAdvert(_) => resp::NEW_ADVERT,
            Response::TelemetryResponse { .. } => resp::TELEMETRY_RESPONSE,
            Response::BinaryResponse { .. } => resp::BINARY_RESPONSE,
            Response::PathDiscoveryResponse { .. } => resp::PATH_DISCOVERY_RESPONSE,
            Response::ControlData { .. } => resp::CONTROL_DATA,
            Response::ContactDeleted { .. } => resp::CONTACT_DELETED,
            Response::ContactsFull => resp::CONTACTS_FULL,
            Response::Other { code, .. } => *code,
        }
    }
}

// -------------------------------------------------------------------------------------- display

impl fmt::Display for Contact<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "key={} type={} flags={} path_len={} name={} last_advert={} lat={} lon={} lastmod={}",
            Hex(&self.key),
            self.kind,
            self.flags,
            self.path_len,
            Text(fixed_str(&self.name)),
            self.last_advert,
            self.lat,
            self.lon,
            self.lastmod
        )?;
        if !self.ext.is_empty() {
            write!(f, " ext={}", Hex(self.ext))?;
        }
        Ok(())
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stats::Core {
                battery_mv,
                uptime_s,
                err_flags,
                queue_len,
            } => write!(
                f,
                "core battery={battery_mv}mV uptime={uptime_s}s err_flags={err_flags:#06x} queue={queue_len}"
            ),
            Stats::Radio {
                noise_floor,
                last_rssi,
                last_snr4,
                tx_air_s,
                rx_air_s,
            } => write!(
                f,
                "radio noise_floor={noise_floor} rssi={last_rssi} snr4={last_snr4} tx_air={tx_air_s}s rx_air={rx_air_s}s"
            ),
            Stats::Packets {
                recv,
                sent,
                sent_flood,
                sent_direct,
                recv_flood,
                recv_direct,
                recv_errors,
            } => write!(
                f,
                "packets recv={recv} sent={sent} sent_flood={sent_flood} sent_direct={sent_direct} recv_flood={recv_flood} recv_direct={recv_direct} recv_errors={recv_errors}"
            ),
        }
    }
}

/// ` reserved=N` only if the reserved byte is non-zero.
fn reserved(f: &mut fmt::Formatter<'_>, r: u8) -> fmt::Result {
    if r != 0 {
        write!(f, " reserved={r}")?;
    }
    Ok(())
}

impl fmt::Display for Response<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Response::Ok => f.write_str("OK"),
            Response::Err(c) => write!(f, "ERR {c}"),
            Response::ContactsStart { total } => write!(f, "CONTACTS_START total={total}"),
            Response::Contact(c) => write!(f, "CONTACT {c}"),
            Response::EndOfContacts { lastmod } => write!(f, "END_OF_CONTACTS lastmod={lastmod}"),
            Response::SelfInfo(s) => write!(
                f,
                "SELF_INFO type={} tx={}/{} key={} lat={} lon={} manual_add={} freq={}kHz bw={}Hz sf={} cr={} name={}",
                s.adv_type,
                s.tx_power,
                s.max_tx_power,
                Hex(&s.key),
                s.lat,
                s.lon,
                s.manual_add,
                s.freq_khz,
                s.bw_hz,
                s.sf,
                s.cr,
                Text(s.node_name)
            ),
            Response::Sent {
                flood,
                ack,
                timeout_ms,
            } => write!(
                f,
                "SENT flood={flood} ack={} timeout={timeout_ms}ms",
                Hex(ack)
            ),
            Response::ContactMsgLegacy {
                prefix,
                path_len,
                txt_type,
                timestamp,
                text,
            } => write!(
                f,
                "CONTACT_MSG_LEGACY prefix={} path_len={path_len} type={txt_type} ts={timestamp} text={}",
                Hex(prefix),
                Hex(text)
            ),
            Response::ChannelMsgLegacy {
                idx,
                path_len,
                txt_type,
                timestamp,
                text,
            } => write!(
                f,
                "CHANNEL_MSG_LEGACY idx={idx} path_len={path_len} type={txt_type} ts={timestamp} text={}",
                Text(text)
            ),
            Response::CurrTime { unix_s } => write!(f, "CURR_TIME {unix_s}"),
            Response::NoMoreMessages => f.write_str("NO_MORE_MESSAGES"),
            Response::ExportContact { raw } => write!(f, "EXPORT_CONTACT raw={}", Hex(raw)),
            Response::BattAndStorage {
                battery_mv,
                used_kb,
                total_kb,
            } => write!(
                f,
                "BATT_AND_STORAGE battery={battery_mv}mV used={used_kb}kB total={total_kb}kB"
            ),
            Response::DeviceInfo(d) => {
                write!(
                    f,
                    "DEVICE_INFO fw={} contacts={} channels={} pin={} build={} maker={} version={} repeat={} path_hash_mode={}",
                    d.fw_ver,
                    u16::from(d.max_contacts_half) * 2,
                    d.max_channels,
                    d.ble_pin,
                    Text(fixed_str(&d.build_date)),
                    Text(fixed_str(&d.manufacturer)),
                    Text(fixed_str(&d.version)),
                    d.repeat,
                    d.path_hash_mode
                )?;
                if !d.ext.is_empty() {
                    write!(f, " ext={}", Hex(d.ext))?;
                }
                Ok(())
            }
            Response::PrivateKey { .. } => f.write_str("PRIVATE_KEY <64 bytes>"),
            Response::Disabled => f.write_str("DISABLED"),
            Response::ContactMsg {
                snr4,
                prefix,
                path_len,
                txt_type,
                timestamp,
                text,
                ..
            } => write!(
                f,
                "CONTACT_MSG snr4={snr4} prefix={} path_len={path_len} type={txt_type} ts={timestamp} text={}",
                Hex(prefix),
                Hex(text)
            ),
            Response::ChannelMsg {
                snr4,
                idx,
                path_len,
                txt_type,
                timestamp,
                text,
                ..
            } => write!(
                f,
                "CHANNEL_MSG snr4={snr4} idx={idx} path_len={path_len} type={txt_type} ts={timestamp} text={}",
                Text(text)
            ),
            Response::ChannelInfo { idx, name, secret } => write!(
                f,
                "CHANNEL_INFO {idx} name={} secret={}",
                Text(fixed_str(name)),
                Hex(secret)
            ),
            Response::SignStart {
                reserved: r,
                max_len,
            } => {
                write!(f, "SIGN_START max_len={max_len}")?;
                reserved(f, *r)
            }
            Response::Signature { signature } => write!(f, "SIGNATURE {}", Hex(signature)),
            Response::CustomVars { vars } => write!(f, "CUSTOM_VARS {}", Text(vars)),
            Response::AdvertPath {
                recv_ts,
                path_len,
                path,
            } => write!(
                f,
                "ADVERT_PATH recv_ts={recv_ts} path={path_len:02x}:{}",
                Hex(path)
            ),
            Response::TuningParams {
                rx_delay,
                airtime_factor,
            } => write!(
                f,
                "TUNING_PARAMS rx_delay={rx_delay}/1000 airtime_factor={airtime_factor}/1000"
            ),
            Response::Stats(s) => write!(f, "STATS {s}"),
            Response::AutoaddConfig { config, max_hops } => {
                write!(f, "AUTOADD_CONFIG config={config:#04x} max_hops={max_hops}")
            }
            Response::AllowedRepeatFreq { ranges } => {
                f.write_str("ALLOWED_REPEAT_FREQ")?;
                for (lo, hi) in freq_ranges(ranges) {
                    write!(f, " {lo}-{hi}kHz")?;
                }
                Ok(())
            }
            Response::ChannelData {
                snr4,
                idx,
                path_len,
                data_type,
                data,
                ..
            } => write!(
                f,
                "CHANNEL_DATA snr4={snr4} idx={idx} path_len={path_len} type={data_type:04x} data={}",
                Hex(data)
            ),
            Response::DefaultFloodScope { scope: None } => f.write_str("DEFAULT_FLOOD_SCOPE none"),
            Response::DefaultFloodScope {
                scope: Some((name, key)),
            } => write!(
                f,
                "DEFAULT_FLOOD_SCOPE name={} key={}",
                Text(fixed_str(name)),
                Hex(key)
            ),
            Response::CliReply { text } => write!(f, "CLI_REPLY {}", Text(text)),
            Response::Advert { key } => write!(f, "ADVERT key={}", Hex(key)),
            Response::PathUpdated { key } => write!(f, "PATH_UPDATED key={}", Hex(key)),
            Response::SendConfirmed { ack, rtt_ms } => {
                write!(f, "SEND_CONFIRMED ack={} rtt={rtt_ms}ms", Hex(ack))
            }
            Response::MsgWaiting => f.write_str("MSG_WAITING"),
            Response::RawData {
                snr4,
                rssi,
                reserved: r,
                payload,
            } => write!(
                f,
                "RAW_DATA snr4={snr4} rssi={rssi} reserved={r:#04x} payload={}",
                Hex(payload)
            ),
            Response::LoginSuccess {
                permissions,
                prefix,
                server,
            } => {
                write!(
                    f,
                    "LOGIN_SUCCESS permissions={permissions} prefix={}",
                    Hex(prefix)
                )?;
                match server {
                    Some(s) => write!(
                        f,
                        " server_time={} acl={} fw_level={}",
                        s.server_time, s.acl, s.fw_level
                    ),
                    None => f.write_str(" legacy"),
                }
            }
            Response::LoginFail {
                reserved: r,
                prefix,
            } => {
                write!(f, "LOGIN_FAIL prefix={}", Hex(prefix))?;
                reserved(f, *r)
            }
            Response::StatusResponse {
                reserved: r,
                prefix,
                data,
            } => {
                write!(
                    f,
                    "STATUS_RESPONSE prefix={} data={}",
                    Hex(prefix),
                    Hex(data)
                )?;
                reserved(f, *r)
            }
            Response::LogRxData { snr4, rssi, raw } => {
                write!(f, "LOG_RX_DATA snr4={snr4} rssi={rssi} raw={}", Hex(raw))
            }
            Response::TraceData {
                reserved: r,
                flags,
                tag,
                auth,
                hashes,
                snrs,
                final_snr4,
            } => {
                write!(
                    f,
                    "TRACE_DATA flags={flags} tag={tag} auth={auth} hashes={} snrs={} final_snr4={final_snr4}",
                    Hex(hashes),
                    Hex(snrs)
                )?;
                reserved(f, *r)
            }
            Response::NewAdvert(c) => write!(f, "NEW_ADVERT {c}"),
            Response::TelemetryResponse {
                reserved: r,
                prefix,
                lpp,
            } => {
                write!(
                    f,
                    "TELEMETRY_RESPONSE prefix={} lpp={}",
                    Hex(prefix),
                    Hex(lpp)
                )?;
                reserved(f, *r)
            }
            Response::BinaryResponse {
                reserved: r,
                tag,
                data,
            } => {
                write!(f, "BINARY_RESPONSE tag={} data={}", Hex(tag), Hex(data))?;
                reserved(f, *r)
            }
            Response::PathDiscoveryResponse {
                reserved: r,
                prefix,
                out_path_len,
                out_path,
                in_path_len,
                in_path,
            } => {
                write!(
                    f,
                    "PATH_DISCOVERY_RESPONSE prefix={} out={out_path_len:02x}:{} in={in_path_len:02x}:{}",
                    Hex(prefix),
                    Hex(out_path),
                    Hex(in_path)
                )?;
                reserved(f, *r)
            }
            Response::ControlData {
                snr4,
                rssi,
                path_len,
                payload,
            } => write!(
                f,
                "CONTROL_DATA snr4={snr4} rssi={rssi} path_len={path_len} payload={}",
                Hex(payload)
            ),
            Response::ContactDeleted { key } => write!(f, "CONTACT_DELETED key={}", Hex(key)),
            Response::ContactsFull => f.write_str("CONTACTS_FULL"),
            Response::Other { code, body } => {
                write!(f, "OTHER code={code:#04x} body={}", Hex(body))
            }
        }
    }
}
