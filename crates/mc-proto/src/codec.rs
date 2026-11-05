//! Trames du protocole compagnon MeshCore, dans le sous-ensemble de `docs/spec/companion.md`.
//!
//! Décodage zéro-copie et canonique : `encode(decode(x)) == x` pour toute trame acceptée. Les
//! champs de taille fixe sont gardés tels quels (fin d'un nom, octets d'un chemin au-delà de sa
//! longueur), même quand l'appareil y laisse des octets sans signification.

use core::fmt;
use mc_core::{Hex, Overflow, Reader, Truncated, Writer};

/// Taille maximale d'une trame (`MAX_FRAME_SIZE` du firmware).
pub const FRAME_MAX: usize = 176;
/// Version du protocole annoncée dans DEVICE_QUERY : messages reçus au format V3.
pub const APP_VER: u8 = 3;
/// `path_len` d'un chemin inconnu, ou d'une inondation.
pub const PATH_UNKNOWN: u8 = 0xFF;
/// Type d'un contact « compagnon » (`ADV_TYPE_CHAT`).
pub const ADV_TYPE_CHAT: u8 = 1;

/// Codes des commandes (app → appareil).
pub mod cmd {
    pub const APP_START: u8 = 0x01;
    pub const SEND_TXT_MSG: u8 = 0x02;
    pub const GET_CONTACTS: u8 = 0x04;
    pub const SET_DEVICE_TIME: u8 = 0x06;
    pub const SEND_SELF_ADVERT: u8 = 0x07;
    pub const ADD_UPDATE_CONTACT: u8 = 0x09;
    pub const SYNC_NEXT_MESSAGE: u8 = 0x0A;
    pub const SET_RADIO_PARAMS: u8 = 0x0B;
    pub const RESET_PATH: u8 = 0x0D;
    pub const DEVICE_QUERY: u8 = 0x16;
    pub const GET_CHANNEL: u8 = 0x1F;
    pub const SET_CHANNEL: u8 = 0x20;
    pub const SEND_CHANNEL_DATA: u8 = 0x3E;
    pub const SET_DEFAULT_FLOOD_SCOPE: u8 = 0x3F;
    pub const GET_DEFAULT_FLOOD_SCOPE: u8 = 0x40;
    pub const GET_DEVICE_TIME: u8 = 0x05;
    pub const EXPORT_PRIVATE_KEY: u8 = 0x17;
    pub const IMPORT_PRIVATE_KEY: u8 = 0x18;
    pub const SEND_RAW_PACKET: u8 = 0x41;
    pub const REMOVE_CONTACT: u8 = 0x0F;
}

/// Codes des réponses et des messages spontanés (appareil → app).
pub mod resp {
    pub const OK: u8 = 0x00;
    pub const ERR: u8 = 0x01;
    pub const CONTACTS_START: u8 = 0x02;
    pub const CONTACT: u8 = 0x03;
    pub const END_OF_CONTACTS: u8 = 0x04;
    pub const SELF_INFO: u8 = 0x05;
    pub const SENT: u8 = 0x06;
    pub const NO_MORE_MESSAGES: u8 = 0x0A;
    pub const DEVICE_INFO: u8 = 0x0D;
    pub const CONTACT_MSG_RECV_V3: u8 = 0x10;
    pub const CHANNEL_MSG_RECV_V3: u8 = 0x11;
    pub const CHANNEL_INFO: u8 = 0x12;
    pub const CHANNEL_DATA_RECV: u8 = 0x1B;
    pub const DEFAULT_FLOOD_SCOPE: u8 = 0x1C;
    pub const CURR_TIME: u8 = 0x09;
    pub const PRIVATE_KEY: u8 = 0x0E;
    pub const DISABLED: u8 = 0x0F;
    pub const LOG_RX_DATA: u8 = 0x88;
    pub const ADVERT: u8 = 0x80;
    pub const PATH_UPDATED: u8 = 0x81;
    pub const SEND_CONFIRMED: u8 = 0x82;
    pub const MSG_WAITING: u8 = 0x83;
    pub const NEW_ADVERT: u8 = 0x8A;
    pub const CONTACT_DELETED: u8 = 0x8F;
    pub const CONTACTS_FULL: u8 = 0x90;
}

/// Codes d'erreur de l'appareil (réponse ERR).
pub mod err {
    pub const UNSUPPORTED: u8 = 1;
    pub const NOT_FOUND: u8 = 2;
    pub const TABLE_FULL: u8 = 3;
    pub const BAD_STATE: u8 = 4;
    pub const FILE_IO: u8 = 5;
    pub const ILLEGAL_ARG: u8 = 6;
}

/// Trame refusée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Plus courte que son corps fixe, ou vide.
    Truncated,
    /// Octets en trop, ou longueur annoncée fausse.
    Length,
    /// Valeur hors de ce que décrit la spec.
    Value,
    /// Code de commande inconnu (une réponse de code inconnu, elle, est [`Response::Other`]).
    Code,
}

impl From<Truncated> for DecodeError {
    fn from(_: Truncated) -> Self {
        DecodeError::Truncated
    }
}

fn end(r: &Reader<'_>) -> Result<(), DecodeError> {
    if r.is_empty() {
        Ok(())
    } else {
        Err(DecodeError::Length)
    }
}

/// Un nom de taille fixe (UTF-8 terminé par 0x00, complété de 0x00) : la partie avant le 0x00.
pub fn fixed_str(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b.get(..n).unwrap_or(&[])
}

/// Écrit `s` dans un champ de taille fixe : tronqué pour garder un 0x00, complété de 0x00.
pub fn to_fixed<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    for (o, b) in out.iter_mut().zip(s.iter().take(N.saturating_sub(1))) {
        *o = *b;
    }
    out
}

// ------------------------------------------------------------------------------------ commandes

/// Une commande de l'app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    DeviceQuery {
        app_ver: u8,
    },
    AppStart {
        reserved: [u8; 7],
        app_name: &'a [u8],
    },
    SetDeviceTime {
        unix_s: u32,
    },
    GetContacts {
        since: Option<u32>,
    },
    AddUpdateContact {
        key: [u8; 32],
        kind: u8,
        flags: u8,
        path_len: u8,
        path: [u8; 64],
        name: [u8; 32],
        last_advert: u32,
    },
    ResetPath {
        key: [u8; 32],
    },
    /// `flood` : inondation dans la portée par défaut ; sinon un seul saut.
    SendSelfAdvert {
        flood: bool,
    },
    SendTxtMsg {
        txt_type: u8,
        attempt: u8,
        timestamp: u32,
        prefix: [u8; 6],
        text: &'a [u8],
    },
    GetChannel {
        idx: u8,
    },
    SetChannel {
        idx: u8,
        name: [u8; 32],
        secret: [u8; 16],
    },
    /// Toujours en inondation (`path_len` 0xFF) dans ce sous-ensemble.
    SendChannelData {
        idx: u8,
        /// `None` : inondation (`path_len` 0xFF) ; sinon `(path_len, chemin)`, 0 = zéro saut.
        path: Option<(u8, &'a [u8])>,
        data_type: u16,
        data: &'a [u8],
    },
    RemoveContact {
        key: [u8; 32],
    },
    SyncNextMessage,
    SetRadioParams {
        freq_khz: u32,
        bw_hz: u32,
        sf: u8,
        cr: u8,
    },
    GetDefaultFloodScope,
    GetDeviceTime,
    ExportPrivateKey,
    ImportPrivateKey {
        key: [u8; 64],
    },
    /// Trame brute émise telle quelle (`docs/spec/companion.md`, commande 65).
    SendRawPacket {
        priority: u8,
        raw: &'a [u8],
    },
    /// `None` efface la portée par défaut.
    SetDefaultFloodScope {
        scope: Option<([u8; 31], [u8; 16])>,
    },
}

impl<'a> Command<'a> {
    /// Écrit la trame dans `out`. Renvoie sa longueur.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let mut w = Writer::new(out);
        match self {
            Command::DeviceQuery { app_ver } => {
                w.put_u8(cmd::DEVICE_QUERY)?;
                w.put_u8(*app_ver)?;
            }
            Command::AppStart { reserved, app_name } => {
                w.put_u8(cmd::APP_START)?;
                w.put(reserved)?;
                w.put(app_name)?;
            }
            Command::SetDeviceTime { unix_s } => {
                w.put_u8(cmd::SET_DEVICE_TIME)?;
                w.put_u32_le(*unix_s)?;
            }
            Command::GetContacts { since } => {
                w.put_u8(cmd::GET_CONTACTS)?;
                if let Some(s) = since {
                    w.put_u32_le(*s)?;
                }
            }
            Command::AddUpdateContact {
                key,
                kind,
                flags,
                path_len,
                path,
                name,
                last_advert,
            } => {
                w.put_u8(cmd::ADD_UPDATE_CONTACT)?;
                w.put(key)?;
                w.put_u8(*kind)?;
                w.put_u8(*flags)?;
                w.put_u8(*path_len)?;
                w.put(path)?;
                w.put(name)?;
                w.put_u32_le(*last_advert)?;
            }
            Command::ResetPath { key } => {
                w.put_u8(cmd::RESET_PATH)?;
                w.put(key)?;
            }
            Command::SendSelfAdvert { flood } => {
                w.put_u8(cmd::SEND_SELF_ADVERT)?;
                w.put_u8(u8::from(*flood))?;
            }
            Command::SendTxtMsg {
                txt_type,
                attempt,
                timestamp,
                prefix,
                text,
            } => {
                w.put_u8(cmd::SEND_TXT_MSG)?;
                w.put_u8(*txt_type)?;
                w.put_u8(*attempt)?;
                w.put_u32_le(*timestamp)?;
                w.put(prefix)?;
                w.put(text)?;
            }
            Command::GetChannel { idx } => {
                w.put_u8(cmd::GET_CHANNEL)?;
                w.put_u8(*idx)?;
            }
            Command::SetChannel { idx, name, secret } => {
                w.put_u8(cmd::SET_CHANNEL)?;
                w.put_u8(*idx)?;
                w.put(name)?;
                w.put(secret)?;
            }
            Command::SendChannelData {
                idx,
                path,
                data_type,
                data,
            } => {
                w.put_u8(cmd::SEND_CHANNEL_DATA)?;
                w.put_u8(*idx)?;
                match path {
                    None => w.put_u8(PATH_UNKNOWN)?,
                    Some((len, bytes)) => {
                        w.put_u8(*len)?;
                        w.put(bytes)?;
                    }
                }
                w.put_u16_le(*data_type)?;
                w.put(data)?;
            }
            Command::RemoveContact { key } => {
                w.put_u8(cmd::REMOVE_CONTACT)?;
                w.put(key)?;
            }
            Command::SyncNextMessage => w.put_u8(cmd::SYNC_NEXT_MESSAGE)?,
            Command::SetRadioParams {
                freq_khz,
                bw_hz,
                sf,
                cr,
            } => {
                w.put_u8(cmd::SET_RADIO_PARAMS)?;
                w.put_u32_le(*freq_khz)?;
                w.put_u32_le(*bw_hz)?;
                w.put_u8(*sf)?;
                w.put_u8(*cr)?;
            }
            Command::GetDefaultFloodScope => w.put_u8(cmd::GET_DEFAULT_FLOOD_SCOPE)?,
            Command::GetDeviceTime => w.put_u8(cmd::GET_DEVICE_TIME)?,
            Command::ExportPrivateKey => w.put_u8(cmd::EXPORT_PRIVATE_KEY)?,
            Command::ImportPrivateKey { key } => {
                w.put_u8(cmd::IMPORT_PRIVATE_KEY)?;
                w.put(key)?;
            }
            Command::SendRawPacket { priority, raw } => {
                w.put_u8(cmd::SEND_RAW_PACKET)?;
                w.put_u8(*priority)?;
                w.put(raw)?;
            }
            Command::SetDefaultFloodScope { scope } => {
                w.put_u8(cmd::SET_DEFAULT_FLOOD_SCOPE)?;
                if let Some((name, key)) = scope {
                    w.put(name)?;
                    w.put(key)?;
                }
            }
        }
        if w.position() > FRAME_MAX {
            return Err(Overflow);
        }
        Ok(w.position())
    }

    /// Décode une commande (côté appareil : le faux réseau, le fuzz).
    pub fn decode(frame: &'a [u8]) -> Result<Self, DecodeError> {
        if frame.len() > FRAME_MAX {
            return Err(DecodeError::Length);
        }
        let mut r = Reader::new(frame);
        let c = match r.u8()? {
            cmd::DEVICE_QUERY => Command::DeviceQuery { app_ver: r.u8()? },
            cmd::APP_START => Command::AppStart {
                reserved: *r.array::<7>()?,
                app_name: r.rest(),
            },
            cmd::SET_DEVICE_TIME => Command::SetDeviceTime {
                unix_s: r.u32_le()?,
            },
            cmd::GET_CONTACTS => Command::GetContacts {
                since: if r.is_empty() {
                    None
                } else {
                    Some(r.u32_le()?)
                },
            },
            cmd::ADD_UPDATE_CONTACT => Command::AddUpdateContact {
                key: *r.array::<32>()?,
                kind: r.u8()?,
                flags: r.u8()?,
                path_len: r.u8()?,
                path: *r.array::<64>()?,
                name: *r.array::<32>()?,
                last_advert: r.u32_le()?,
            },
            cmd::RESET_PATH => Command::ResetPath {
                key: *r.array::<32>()?,
            },
            cmd::SEND_SELF_ADVERT => Command::SendSelfAdvert {
                flood: match r.u8()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::Value),
                },
            },
            cmd::SEND_TXT_MSG => {
                let txt_type = r.u8()?;
                let attempt = r.u8()?;
                let timestamp = r.u32_le()?;
                let prefix = *r.array::<6>()?;
                let text = r.rest();
                if text.is_empty() {
                    return Err(DecodeError::Truncated);
                }
                Command::SendTxtMsg {
                    txt_type,
                    attempt,
                    timestamp,
                    prefix,
                    text,
                }
            }
            cmd::GET_CHANNEL => Command::GetChannel { idx: r.u8()? },
            cmd::SET_CHANNEL => Command::SetChannel {
                idx: r.u8()?,
                name: *r.array::<32>()?,
                secret: *r.array::<16>()?,
            },
            cmd::SEND_CHANNEL_DATA => {
                let idx = r.u8()?;
                let path_len = r.u8()?;
                let path = if path_len == PATH_UNKNOWN {
                    None
                } else {
                    // Taille de hash 1 à 3 octets, au plus 64 octets de chemin (M:1198-1237).
                    let size = usize::from(path_len >> 6) + 1;
                    let bytes = usize::from(path_len & 63) * size;
                    if size == 4 || bytes > 64 {
                        return Err(DecodeError::Value);
                    }
                    Some((path_len, r.take(bytes)?))
                };
                let data_type = r.u16_le()?;
                if data_type == 0 {
                    return Err(DecodeError::Value);
                }
                let data = r.rest();
                if data.is_empty() {
                    return Err(DecodeError::Truncated);
                }
                Command::SendChannelData {
                    idx,
                    path,
                    data_type,
                    data,
                }
            }
            cmd::REMOVE_CONTACT => Command::RemoveContact {
                key: *r.array::<32>()?,
            },
            cmd::SYNC_NEXT_MESSAGE => Command::SyncNextMessage,
            cmd::SET_RADIO_PARAMS => Command::SetRadioParams {
                freq_khz: r.u32_le()?,
                bw_hz: r.u32_le()?,
                sf: r.u8()?,
                cr: r.u8()?,
            },
            cmd::GET_DEFAULT_FLOOD_SCOPE => Command::GetDefaultFloodScope,
            cmd::GET_DEVICE_TIME => Command::GetDeviceTime,
            cmd::EXPORT_PRIVATE_KEY => Command::ExportPrivateKey,
            cmd::IMPORT_PRIVATE_KEY => Command::ImportPrivateKey {
                key: *r.array::<64>()?,
            },
            cmd::SEND_RAW_PACKET => {
                let priority = r.u8()?;
                let raw = r.rest();
                // Au moins un en-tête et un path_len (le C++ exige len ≥ 4, M:2045).
                if raw.len() < 2 {
                    return Err(DecodeError::Truncated);
                }
                Command::SendRawPacket { priority, raw }
            }
            cmd::SET_DEFAULT_FLOOD_SCOPE => Command::SetDefaultFloodScope {
                scope: if r.is_empty() {
                    None
                } else {
                    Some((*r.array::<31>()?, *r.array::<16>()?))
                },
            },
            _ => return Err(DecodeError::Code),
        };
        end(&r)?;
        Ok(c)
    }
}

// ------------------------------------------------------------------------------------ réponses

/// Contact d'appareil (CONTACT, NEW_ADVERT).
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
    /// Champs ajoutés par un firmware plus récent, gardés tels quels.
    pub ext: &'a [u8],
}

impl<'a> Contact<'a> {
    fn read(r: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            key: *r.array::<32>()?,
            kind: r.u8()?,
            flags: r.u8()?,
            path_len: r.u8()?,
            path: *r.array::<64>()?,
            name: *r.array::<32>()?,
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

/// DEVICE_INFO.
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

/// SELF_INFO.
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

/// Une réponse ou un message spontané de l'appareil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response<'a> {
    Ok,
    Err(u8),
    ContactsStart {
        total: u32,
    },
    Contact(Contact<'a>),
    EndOfContacts {
        lastmod: u32,
    },
    SelfInfo(SelfInfo<'a>),
    Sent {
        flood: bool,
        ack: [u8; 4],
        timeout_ms: u32,
    },
    NoMoreMessages,
    DeviceInfo(DeviceInfo<'a>),
    ContactMsg {
        snr4: i8,
        reserved: [u8; 2],
        prefix: [u8; 6],
        path_len: u8,
        txt_type: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    ChannelMsg {
        snr4: i8,
        reserved: [u8; 2],
        idx: u8,
        path_len: u8,
        txt_type: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    ChannelInfo {
        idx: u8,
        name: [u8; 32],
        secret: [u8; 16],
    },
    ChannelData {
        snr4: i8,
        reserved: [u8; 2],
        idx: u8,
        path_len: u8,
        data_type: u16,
        data: &'a [u8],
    },
    DefaultFloodScope {
        scope: Option<([u8; 31], [u8; 16])>,
    },
    Advert {
        key: [u8; 32],
    },
    PathUpdated {
        key: [u8; 32],
    },
    SendConfirmed {
        ack: [u8; 4],
        rtt_ms: u32,
    },
    MsgWaiting,
    NewAdvert(Contact<'a>),
    ContactDeleted {
        key: [u8; 32],
    },
    ContactsFull,
    CurrTime {
        unix_s: u32,
    },
    PrivateKey {
        key: [u8; 64],
    },
    /// Fonction désactivée à la compilation (export ou import de clé).
    Disabled,
    /// Chaque trame reçue par la radio (notification 0x88).
    LogRxData {
        snr4: i8,
        rssi: i8,
        raw: &'a [u8],
    },
    /// Code hors du sous-ensemble : ignoré par la session.
    Other {
        code: u8,
        body: &'a [u8],
    },
}

/// Codes que ce module décode (les autres deviennent [`Response::Other`]).
const KNOWN: [u8; 25] = [
    resp::CURR_TIME,
    resp::PRIVATE_KEY,
    resp::DISABLED,
    resp::LOG_RX_DATA,
    resp::OK,
    resp::ERR,
    resp::CONTACTS_START,
    resp::CONTACT,
    resp::END_OF_CONTACTS,
    resp::SELF_INFO,
    resp::SENT,
    resp::NO_MORE_MESSAGES,
    resp::DEVICE_INFO,
    resp::CONTACT_MSG_RECV_V3,
    resp::CHANNEL_MSG_RECV_V3,
    resp::CHANNEL_INFO,
    resp::CHANNEL_DATA_RECV,
    resp::DEFAULT_FLOOD_SCOPE,
    resp::ADVERT,
    resp::PATH_UPDATED,
    resp::SEND_CONFIRMED,
    resp::MSG_WAITING,
    resp::NEW_ADVERT,
    resp::CONTACT_DELETED,
    resp::CONTACTS_FULL,
];

fn i8_of(b: u8) -> i8 {
    i8::from_le_bytes([b])
}

impl<'a> Response<'a> {
    /// Vrai pour un message spontané (code ≥ 0x80), qui ne répond à aucune commande.
    pub fn is_push(frame: &[u8]) -> bool {
        frame.first().is_some_and(|c| *c >= 0x80)
    }

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
                key: *r.array::<32>()?,
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
                ack: *r.array::<4>()?,
                timeout_ms: r.u32_le()?,
            },
            resp::NO_MORE_MESSAGES => Response::NoMoreMessages,
            resp::DEVICE_INFO => Response::DeviceInfo(DeviceInfo {
                fw_ver: r.u8()?,
                max_contacts_half: r.u8()?,
                max_channels: r.u8()?,
                ble_pin: r.u32_le()?,
                build_date: *r.array::<12>()?,
                manufacturer: *r.array::<40>()?,
                version: *r.array::<20>()?,
                repeat: r.u8()?,
                path_hash_mode: r.u8()?,
                ext: r.rest(),
            }),
            resp::CONTACT_MSG_RECV_V3 => Response::ContactMsg {
                snr4: i8_of(r.u8()?),
                reserved: *r.array::<2>()?,
                prefix: *r.array::<6>()?,
                path_len: r.u8()?,
                txt_type: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            resp::CHANNEL_MSG_RECV_V3 => Response::ChannelMsg {
                snr4: i8_of(r.u8()?),
                reserved: *r.array::<2>()?,
                idx: r.u8()?,
                path_len: r.u8()?,
                txt_type: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            resp::CHANNEL_INFO => Response::ChannelInfo {
                idx: r.u8()?,
                name: *r.array::<32>()?,
                secret: *r.array::<16>()?,
            },
            resp::CHANNEL_DATA_RECV => {
                let snr4 = i8_of(r.u8()?);
                let reserved = *r.array::<2>()?;
                let idx = r.u8()?;
                let path_len = r.u8()?;
                let data_type = r.u16_le()?;
                let len = usize::from(r.u8()?);
                let data = r.take(len)?;
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
                    Some((*r.array::<31>()?, *r.array::<16>()?))
                },
            },
            resp::ADVERT => Response::Advert {
                key: *r.array::<32>()?,
            },
            resp::PATH_UPDATED => Response::PathUpdated {
                key: *r.array::<32>()?,
            },
            resp::SEND_CONFIRMED => Response::SendConfirmed {
                ack: *r.array::<4>()?,
                rtt_ms: r.u32_le()?,
            },
            resp::MSG_WAITING => Response::MsgWaiting,
            resp::NEW_ADVERT => Response::NewAdvert(Contact::read(&mut r)?),
            resp::CONTACT_DELETED => Response::ContactDeleted {
                key: *r.array::<32>()?,
            },
            resp::CONTACTS_FULL => Response::ContactsFull,
            resp::CURR_TIME => Response::CurrTime {
                unix_s: r.u32_le()?,
            },
            resp::PRIVATE_KEY => Response::PrivateKey {
                key: *r.array::<64>()?,
            },
            resp::DISABLED => Response::Disabled,
            resp::LOG_RX_DATA => Response::LogRxData {
                snr4: i8_of(r.u8()?),
                rssi: i8_of(r.u8()?),
                raw: r.rest(),
            },
            code => Response::Other {
                code,
                body: r.rest(),
            },
        };
        end(&r)?;
        Ok(v)
    }

    /// Écrit la trame dans `out`. Renvoie sa longueur.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let mut w = Writer::new(out);
        match self {
            Response::Ok => w.put_u8(resp::OK)?,
            Response::Err(c) => {
                w.put_u8(resp::ERR)?;
                w.put_u8(*c)?;
            }
            Response::ContactsStart { total } => {
                w.put_u8(resp::CONTACTS_START)?;
                w.put_u32_le(*total)?;
            }
            Response::Contact(c) => {
                w.put_u8(resp::CONTACT)?;
                c.write(&mut w)?;
            }
            Response::EndOfContacts { lastmod } => {
                w.put_u8(resp::END_OF_CONTACTS)?;
                w.put_u32_le(*lastmod)?;
            }
            Response::SelfInfo(s) => {
                w.put_u8(resp::SELF_INFO)?;
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
                w.put_u8(resp::SENT)?;
                w.put_u8(u8::from(*flood))?;
                w.put(ack)?;
                w.put_u32_le(*timeout_ms)?;
            }
            Response::NoMoreMessages => w.put_u8(resp::NO_MORE_MESSAGES)?,
            Response::DeviceInfo(d) => {
                w.put_u8(resp::DEVICE_INFO)?;
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
            Response::ContactMsg {
                snr4,
                reserved,
                prefix,
                path_len,
                txt_type,
                timestamp,
                text,
            } => {
                w.put_u8(resp::CONTACT_MSG_RECV_V3)?;
                w.put(&snr4.to_le_bytes())?;
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
                w.put_u8(resp::CHANNEL_MSG_RECV_V3)?;
                w.put(&snr4.to_le_bytes())?;
                w.put(reserved)?;
                w.put_u8(*idx)?;
                w.put_u8(*path_len)?;
                w.put_u8(*txt_type)?;
                w.put_u32_le(*timestamp)?;
                w.put(text)?;
            }
            Response::ChannelInfo { idx, name, secret } => {
                w.put_u8(resp::CHANNEL_INFO)?;
                w.put_u8(*idx)?;
                w.put(name)?;
                w.put(secret)?;
            }
            Response::ChannelData {
                snr4,
                reserved,
                idx,
                path_len,
                data_type,
                data,
            } => {
                w.put_u8(resp::CHANNEL_DATA_RECV)?;
                w.put(&snr4.to_le_bytes())?;
                w.put(reserved)?;
                w.put_u8(*idx)?;
                w.put_u8(*path_len)?;
                w.put_u16_le(*data_type)?;
                w.put_len8(data)?;
            }
            Response::DefaultFloodScope { scope } => {
                w.put_u8(resp::DEFAULT_FLOOD_SCOPE)?;
                if let Some((name, key)) = scope {
                    w.put(name)?;
                    w.put(key)?;
                }
            }
            Response::Advert { key } => {
                w.put_u8(resp::ADVERT)?;
                w.put(key)?;
            }
            Response::PathUpdated { key } => {
                w.put_u8(resp::PATH_UPDATED)?;
                w.put(key)?;
            }
            Response::SendConfirmed { ack, rtt_ms } => {
                w.put_u8(resp::SEND_CONFIRMED)?;
                w.put(ack)?;
                w.put_u32_le(*rtt_ms)?;
            }
            Response::MsgWaiting => w.put_u8(resp::MSG_WAITING)?,
            Response::NewAdvert(c) => {
                w.put_u8(resp::NEW_ADVERT)?;
                c.write(&mut w)?;
            }
            Response::ContactDeleted { key } => {
                w.put_u8(resp::CONTACT_DELETED)?;
                w.put(key)?;
            }
            Response::ContactsFull => w.put_u8(resp::CONTACTS_FULL)?,
            Response::CurrTime { unix_s } => {
                w.put_u8(resp::CURR_TIME)?;
                w.put_u32_le(*unix_s)?;
            }
            Response::PrivateKey { key } => {
                w.put_u8(resp::PRIVATE_KEY)?;
                w.put(key)?;
            }
            Response::Disabled => w.put_u8(resp::DISABLED)?,
            Response::LogRxData { snr4, rssi, raw } => {
                w.put_u8(resp::LOG_RX_DATA)?;
                w.put(&snr4.to_le_bytes())?;
                w.put(&rssi.to_le_bytes())?;
                w.put(raw)?;
            }
            Response::Other { code, body } => {
                // Un code connu ne s'écrit jamais en Other : sa forme décodée serait autre.
                if KNOWN.contains(code) {
                    return Err(Overflow);
                }
                w.put_u8(*code)?;
                w.put(body)?;
            }
        }
        if w.position() > FRAME_MAX {
            return Err(Overflow);
        }
        Ok(w.position())
    }
}

// ------------------------------------------------------------------------------------ affichage

struct Text<'a>(&'a [u8]);

impl fmt::Display for Text<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"")?;
        for c in self.0.utf8_chunks() {
            for ch in c.valid().chars() {
                write!(f, "{}", ch.escape_debug())?;
            }
            for b in c.invalid() {
                write!(f, "\\x{b:02x}")?;
            }
        }
        f.write_str("\"")
    }
}

impl fmt::Display for Command<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::DeviceQuery { app_ver } => write!(f, "DEVICE_QUERY app_ver={app_ver}"),
            Command::AppStart { reserved, app_name } => write!(
                f,
                "APP_START reserved={} app={}",
                Hex(reserved),
                Text(app_name)
            ),
            Command::SetDeviceTime { unix_s } => write!(f, "SET_DEVICE_TIME {unix_s}"),
            Command::GetContacts { since: None } => f.write_str("GET_CONTACTS"),
            Command::GetContacts { since: Some(s) } => write!(f, "GET_CONTACTS since={s}"),
            Command::AddUpdateContact {
                key,
                kind,
                flags,
                path_len,
                path,
                name,
                last_advert,
            } => write!(
                f,
                "ADD_UPDATE_CONTACT key={} type={kind} flags={flags} path_len={path_len} path={} name={} last_advert={last_advert}",
                Hex(key),
                Hex(path),
                Text(fixed_str(name))
            ),
            Command::ResetPath { key } => write!(f, "RESET_PATH key={}", Hex(key)),
            Command::SendSelfAdvert { flood } => write!(f, "SEND_SELF_ADVERT flood={flood}"),
            Command::SendTxtMsg {
                txt_type,
                attempt,
                timestamp,
                prefix,
                text,
            } => write!(
                f,
                "SEND_TXT_MSG type={txt_type} attempt={attempt} ts={timestamp} prefix={} text={}",
                Hex(prefix),
                Hex(text)
            ),
            Command::GetChannel { idx } => write!(f, "GET_CHANNEL {idx}"),
            Command::SetChannel { idx, name, secret } => write!(
                f,
                "SET_CHANNEL {idx} name={} secret={}",
                Text(fixed_str(name)),
                Hex(secret)
            ),
            Command::SendChannelData {
                idx,
                path,
                data_type,
                data,
            } => {
                write!(
                    f,
                    "SEND_CHANNEL_DATA {idx} type={data_type:04x} data={}",
                    Hex(data)
                )?;
                if let Some((len, bytes)) = path {
                    write!(f, " path={len:02x}:{}", Hex(bytes))?;
                }
                Ok(())
            }
            Command::RemoveContact { key } => write!(f, "REMOVE_CONTACT key={}", Hex(key)),
            Command::SyncNextMessage => f.write_str("SYNC_NEXT_MESSAGE"),
            Command::SetRadioParams {
                freq_khz,
                bw_hz,
                sf,
                cr,
            } => write!(
                f,
                "SET_RADIO_PARAMS freq={freq_khz}kHz bw={bw_hz}Hz sf={sf} cr={cr}"
            ),
            Command::GetDefaultFloodScope => f.write_str("GET_DEFAULT_FLOOD_SCOPE"),
            Command::GetDeviceTime => f.write_str("GET_DEVICE_TIME"),
            Command::ExportPrivateKey => f.write_str("EXPORT_PRIVATE_KEY"),
            Command::ImportPrivateKey { .. } => f.write_str("IMPORT_PRIVATE_KEY <64 octets>"),
            Command::SendRawPacket { priority, raw } => {
                write!(f, "SEND_RAW_PACKET prio={priority} raw={}", Hex(raw))
            }
            Command::SetDefaultFloodScope { scope: None } => {
                f.write_str("SET_DEFAULT_FLOOD_SCOPE none")
            }
            Command::SetDefaultFloodScope {
                scope: Some((name, key)),
            } => write!(
                f,
                "SET_DEFAULT_FLOOD_SCOPE name={} key={}",
                Text(fixed_str(name)),
                Hex(key)
            ),
        }
    }
}

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
            Response::NoMoreMessages => f.write_str("NO_MORE_MESSAGES"),
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
            Response::Advert { key } => write!(f, "ADVERT key={}", Hex(key)),
            Response::PathUpdated { key } => write!(f, "PATH_UPDATED key={}", Hex(key)),
            Response::SendConfirmed { ack, rtt_ms } => {
                write!(f, "SEND_CONFIRMED ack={} rtt={rtt_ms}ms", Hex(ack))
            }
            Response::MsgWaiting => f.write_str("MSG_WAITING"),
            Response::NewAdvert(c) => write!(f, "NEW_ADVERT {c}"),
            Response::ContactDeleted { key } => write!(f, "CONTACT_DELETED key={}", Hex(key)),
            Response::ContactsFull => f.write_str("CONTACTS_FULL"),
            Response::CurrTime { unix_s } => write!(f, "CURR_TIME {unix_s}"),
            Response::PrivateKey { .. } => f.write_str("PRIVATE_KEY <64 octets>"),
            Response::Disabled => f.write_str("DISABLED"),
            Response::LogRxData { snr4, rssi, raw } => {
                write!(f, "LOG_RX_DATA snr4={snr4} rssi={rssi} raw={}", Hex(raw))
            }
            Response::Other { code, body } => {
                write!(f, "OTHER code={code:#04x} body={}", Hex(body))
            }
        }
    }
}
