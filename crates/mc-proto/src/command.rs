//! Commandes de l'app (`docs/spec/companion.md` §2), dans les deux modes de décodage :
//!
//! - [`Command::decode`] : **strict** (app, outils, tests), canonique ;
//! - [`Command::decode_device`] : ce qu'accepte `MyMesh::handleCmdFrame` au stade de l'analyse
//!   (§10, ADR-0002) : champs finaux optionnels, octets en trop ignorés, préconditions de
//!   longueur exactes du C++, octets lus au-delà de la trame valant 0.
//!
//! Toute commande acceptée par `decode_device` s'écrit avec [`Command::encode`] en une trame que
//! `decode` relit à l'identique. Les contrôles que le C++ fait après l'analyse (bornes → ERR 6,
//! contact ou canal inconnu → ERR 2…) appartiennent à la machine d'état, pas au codec.

use core::fmt;
use mc_core::{Hex, Overflow, Reader, Writer};

use crate::codec::{
    DecodeError, DeviceReject, FRAME_MAX, MAX_PATH, PATH_UNKNOWN, Text, cmd, end, fixed_str, i8_of,
    path_bytes, read_path,
};

/// Portée d'inondation temporaire, en RAM (commande 54, M:1978-1988).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloodScopeKey {
    /// `[1]` = 0 sans clé : efface la portée temporaire.
    Clear,
    /// `[1]` = 0 suivi de la clé de région (16 octets).
    Key([u8; 16]),
    /// `[1]` = 1 (v12+) : inondations sans portée.
    Unscoped,
}

/// Une commande de l'app. Les positions `[i]` comptent le code en `[0]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// Commande 1 : `[1..7]` réservés, `[8..]` nom de l'app (peut être vide).
    AppStart {
        reserved: [u8; 7],
        app_name: &'a [u8],
    },
    /// Commande 2 : texte non vide.
    SendTxtMsg {
        txt_type: u8,
        attempt: u8,
        timestamp: u32,
        prefix: [u8; 6],
        text: &'a [u8],
    },
    /// Commande 3 : texte éventuellement vide.
    SendChannelTxtMsg {
        txt_type: u8,
        idx: u8,
        timestamp: u32,
        text: &'a [u8],
    },
    /// Commande 4. Sans `since`, tous les contacts.
    GetContacts { since: Option<u32> },
    /// Commande 5.
    GetDeviceTime,
    /// Commande 6.
    SetDeviceTime { unix_s: u32 },
    /// Commande 7. `Some(true)` : inondation dans la portée par défaut ; `Some(false)` ou `None`
    /// (octet absent) : zéro saut.
    SendSelfAdvert { flood: Option<bool> },
    /// Commande 8 : nom non vide (l'appareil le tronque à 31 octets).
    SetAdvertName { name: &'a [u8] },
    /// Commande 9 : contact (§3.2) ; `lat_lon` puis `lastmod` optionnels (`lastmod` exige
    /// `lat_lon`).
    AddUpdateContact {
        key: [u8; 32],
        kind: u8,
        flags: u8,
        path_len: u8,
        path: [u8; 64],
        name: [u8; 32],
        last_advert: u32,
        lat_lon: Option<(i32, i32)>,
        lastmod: Option<u32>,
    },
    /// Commande 10.
    SyncNextMessage,
    /// Commande 11. Sans `repeat`, la répétition est désactivée.
    SetRadioParams {
        freq_khz: u32,
        bw_hz: u32,
        sf: u8,
        cr: u8,
        repeat: Option<u8>,
    },
    /// Commande 12.
    SetRadioTxPower { dbm: i8 },
    /// Commande 13.
    ResetPath { key: [u8; 32] },
    /// Commande 14 : degrés × 10⁶ ; `alt` est lue mais ignorée par le C++.
    SetAdvertLatLon {
        lat: i32,
        lon: i32,
        alt: Option<i32>,
    },
    /// Commande 15.
    RemoveContact { key: [u8; 32] },
    /// Commande 16.
    ShareContact { key: [u8; 32] },
    /// Commande 17. `None` : exporte l'appareil lui-même.
    ExportContact { key: Option<[u8; 32]> },
    /// Commande 18 : trame d'annonce brute, au moins 98 octets.
    ImportContact { raw: &'a [u8] },
    /// Commande 19 : `[1..6]` = `"reboot"`.
    Reboot,
    /// Commande 20.
    GetBattAndStorage,
    /// Commande 21 : valeurs × 1000.
    SetTuningParams { rx_delay: u32, airtime_factor: u32 },
    /// Commande 22.
    DeviceQuery { app_ver: u8 },
    /// Commande 23.
    ExportPrivateKey,
    /// Commande 24.
    ImportPrivateKey { key: [u8; 64] },
    /// Commande 25 : chemin valide (taille de hash 1 à 3, 64 octets au plus), charge d'au moins
    /// 4 octets.
    SendRawData {
        path_len: u8,
        path: &'a [u8],
        payload: &'a [u8],
    },
    /// Commande 26 : mot de passe éventuellement vide (15 caractères utiles).
    SendLogin { key: [u8; 32], password: &'a [u8] },
    /// Commande 27.
    SendStatusReq { key: [u8; 32] },
    /// Commande 28.
    HasConnection { key: [u8; 32] },
    /// Commande 29.
    Logout { key: [u8; 32] },
    /// Commande 30.
    GetContactByKey { key: [u8; 32] },
    /// Commande 31.
    GetChannel { idx: u8 },
    /// Commande 32 : secret de 16 octets seulement (une clé de 32 octets reçoit ERR 1).
    SetChannel {
        idx: u8,
        name: [u8; 32],
        secret: [u8; 16],
    },
    /// Commande 33.
    SignStart,
    /// Commande 34 : morceau non vide.
    SignData { data: &'a [u8] },
    /// Commande 35.
    SignFinish,
    /// Commande 36 : hashes non vides, de 2^(`flags & 3`) octets chacun, 64 au plus.
    SendTracePath {
        tag: u32,
        auth: u32,
        flags: u8,
        path: &'a [u8],
    },
    /// Commande 37.
    SetDevicePin { pin: u32 },
    /// Commande 38. Champs optionnels dans l'ordre, chacun exige le précédent.
    SetOtherParams {
        manual_add: u8,
        telemetry: Option<u8>,
        advert_loc_policy: Option<u8>,
        multi_acks: Option<u8>,
    },
    /// Commande 39. `None` : télémétrie de l'appareil lui-même (trame de 4 octets).
    SendTelemetryReq {
        reserved: [u8; 3],
        key: Option<[u8; 32]>,
    },
    /// Commande 40.
    GetCustomVars,
    /// Commande 41 : `"nom:valeur"`, au moins 3 octets.
    SetCustomVar { var: &'a [u8] },
    /// Commande 42 : clé comparée sur ses 7 premiers octets.
    GetAdvertPath { reserved: u8, key: [u8; 32] },
    /// Commande 43.
    GetTuningParams,
    /// Commande 50 : données non vides.
    SendBinaryReq { key: [u8; 32], data: &'a [u8] },
    /// Commande 51 : `[1..5]` = `"reset"`.
    FactoryReset,
    /// Commande 52 : `[1]` = 0, puis la clé.
    SendPathDiscoveryReq { key: [u8; 32] },
    /// Commande 54.
    SetFloodScopeKey { scope: FloodScopeKey },
    /// Commande 55 : charge non vide dont le premier octet a le bit 0x80.
    SendControlData { data: &'a [u8] },
    /// Commande 56. 0 = cœur, 1 = radio, 2 = paquets.
    GetStats { kind: u8 },
    /// Commande 57 : données non vides.
    SendAnonReq { key: [u8; 32], data: &'a [u8] },
    /// Commande 58.
    SetAutoaddConfig { config: u8, max_hops: Option<u8> },
    /// Commande 59.
    GetAutoaddConfig,
    /// Commande 60.
    GetAllowedRepeatFreq,
    /// Commande 61 : `[1]` = 0, puis le mode.
    SetPathHashMode { mode: u8 },
    /// Commande 62.
    SendChannelData {
        idx: u8,
        /// `None` : inondation (`path_len` 0xFF) ; sinon `(path_len, chemin)`, 0 = zéro saut.
        path: Option<(u8, &'a [u8])>,
        data_type: u16,
        data: &'a [u8],
    },
    /// Commande 63. `None` efface la portée par défaut.
    SetDefaultFloodScope { scope: Option<([u8; 31], [u8; 16])> },
    /// Commande 64.
    GetDefaultFloodScope,
    /// Commande 65 : trame brute émise telle quelle, au moins 2 octets.
    SendRawPacket { priority: u8, raw: &'a [u8] },
    /// Commande 66 : commande texte, au moins 2 octets.
    RunCliCommand { cmd: &'a [u8] },
}

// ------------------------------------------------------------------- lecture côté appareil

/// Zéros prêtés quand un chemin déborde de la trame (voir [`device_channel_data`]).
static ZEROS: [u8; MAX_PATH] = [0; MAX_PATH];

/// Octet `i`, ou 0 au-delà de la trame. Le C++ lit alors la suite de `cmd_frame` (M.h:267),
/// tampon qui garde les octets de la trame précédente ; nous lisons des zéros (§10).
fn byte(f: &[u8], i: usize) -> u8 {
    f.get(i).copied().unwrap_or(0)
}

/// `N` octets à partir de `at`, complétés de zéros au-delà de la trame.
fn pad<const N: usize>(f: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    for (o, b) in out.iter_mut().zip(f.iter().skip(at)) {
        *o = *b;
    }
    out
}

fn u32_at(f: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(pad(f, at))
}

fn i32_at(f: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(pad(f, at))
}

/// La fin de la trame à partir de `at` (vide au-delà).
fn tail(f: &[u8], at: usize) -> &[u8] {
    f.get(at..).unwrap_or(&[])
}

/// Hashes d'un TRACE cohérents avec `flags` (M:1813-1818) : `n` multiple de 2^(flags & 3) et
/// au plus 64 hashes.
fn trace_path_ok(flags: u8, n: usize) -> bool {
    let sz = flags & 3;
    (n >> sz) <= MAX_PATH && n.is_multiple_of(1usize << sz)
}

/// SEND_CHANNEL_DATA (M:1198-1237). `len < 4` et `path_len` invalide (hors 0xFF) → ERR 6. Le
/// chemin et `data_type` sont lus sans contrôle de longueur ; la charge est ce qui reste après.
fn device_channel_data(f: &[u8]) -> Result<Command<'_>, DeviceReject> {
    if f.len() < 4 {
        return Err(DeviceReject::ILLEGAL_ARG);
    }
    let idx = byte(f, 1);
    let path_len = byte(f, 2);
    let (path, at) = if path_len == PATH_UNKNOWN {
        (None, 3)
    } else {
        let n = path_bytes(path_len).ok_or(DeviceReject::ILLEGAL_ARG)?;
        // Un chemin qui déborde de la trame entraîne data_type lu hors de la trame, donc 0 : la
        // commande échoue toujours (ERR 2 ou ERR 6), quels que soient les octets du chemin. On
        // prête des zéros pour garder une valeur zéro-copie.
        let bytes = match f.get(3..3 + n) {
            Some(b) => b,
            None => ZEROS.get(..n).unwrap_or(&[]),
        };
        (Some((path_len, bytes)), 3 + n)
    };
    Ok(Command::SendChannelData {
        idx,
        path,
        data_type: u16::from_le_bytes([byte(f, at), byte(f, at + 1)]),
        data: tail(f, at + 2),
    })
}

/// SEND_RAW_DATA (M:1557-1576), trame d'au moins 6 octets : `path_len` invalide (0xFF compris)
/// → ERR 1 ; moins de 4 octets de charge après le chemin → ERR 6.
fn device_raw_data(f: &[u8]) -> Result<Command<'_>, DeviceReject> {
    let path_len = byte(f, 1);
    let n = path_bytes(path_len).ok_or(DeviceReject::UNSUPPORTED)?;
    match (f.get(2..2 + n), f.get(2 + n..)) {
        (Some(path), Some(payload)) if payload.len() >= 4 => Ok(Command::SendRawData {
            path_len,
            path,
            payload,
        }),
        _ => Err(DeviceReject::ILLEGAL_ARG),
    }
}

/// SEND_TRACE_PATH (M:1813-1838), trame de plus de 10 octets : hashes incohérents avec
/// `flags` → ERR 6.
fn device_trace(f: &[u8]) -> Result<Command<'_>, DeviceReject> {
    let flags = byte(f, 9);
    let path = tail(f, 10);
    if !trace_path_ok(flags, path.len()) {
        return Err(DeviceReject::ILLEGAL_ARG);
    }
    Ok(Command::SendTracePath {
        tag: u32_at(f, 1),
        auth: u32_at(f, 5),
        flags,
        path,
    })
}

fn opt_u8(r: &mut Reader<'_>) -> Result<Option<u8>, DecodeError> {
    if r.is_empty() {
        Ok(None)
    } else {
        Ok(Some(r.u8()?))
    }
}

fn opt_u32(r: &mut Reader<'_>) -> Result<Option<u32>, DecodeError> {
    if r.is_empty() {
        Ok(None)
    } else {
        Ok(Some(r.u32_le()?))
    }
}

fn opt_i32(r: &mut Reader<'_>) -> Result<Option<i32>, DecodeError> {
    if r.is_empty() {
        Ok(None)
    } else {
        Ok(Some(r.i32_le()?))
    }
}

fn opt_key(r: &mut Reader<'_>) -> Result<Option<[u8; 32]>, DecodeError> {
    if r.is_empty() {
        Ok(None)
    } else {
        Ok(Some(*r.array()?))
    }
}

/// Le reste, non vide.
fn rest_min<'a>(r: &mut Reader<'a>, min: usize) -> Result<&'a [u8], DecodeError> {
    let b = r.rest();
    if b.len() < min {
        Err(DecodeError::Truncated)
    } else {
        Ok(b)
    }
}

impl<'a> Command<'a> {
    /// Écrit la trame dans `out`. Renvoie sa longueur.
    ///
    /// Échoue si `out` est trop petit, ou si la valeur n'a pas de forme canonique (le décodage
    /// strict de la trame écrite la refuserait ou lirait autre chose : `lastmod` sans position,
    /// chemin de longueur fausse, trame de plus de 176 octets…).
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let n = self.write(out)?;
        match out.get(..n).map(Command::decode) {
            Some(Ok(back)) if back == *self => Ok(n),
            _ => Err(Overflow),
        }
    }

    fn write(&self, out: &mut [u8]) -> Result<usize, Overflow> {
        let mut w = Writer::new(out);
        w.put_u8(self.code())?;
        match self {
            Command::AppStart { reserved, app_name } => {
                w.put(reserved)?;
                w.put(app_name)?;
            }
            Command::SendTxtMsg {
                txt_type,
                attempt,
                timestamp,
                prefix,
                text,
            } => {
                w.put_u8(*txt_type)?;
                w.put_u8(*attempt)?;
                w.put_u32_le(*timestamp)?;
                w.put(prefix)?;
                w.put(text)?;
            }
            Command::SendChannelTxtMsg {
                txt_type,
                idx,
                timestamp,
                text,
            } => {
                w.put_u8(*txt_type)?;
                w.put_u8(*idx)?;
                w.put_u32_le(*timestamp)?;
                w.put(text)?;
            }
            Command::GetContacts { since } => {
                if let Some(s) = since {
                    w.put_u32_le(*s)?;
                }
            }
            Command::SetDeviceTime { unix_s } => w.put_u32_le(*unix_s)?,
            Command::SendSelfAdvert { flood } => {
                if let Some(f) = flood {
                    w.put_u8(u8::from(*f))?;
                }
            }
            Command::SetAdvertName { name } => w.put(name)?,
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
                w.put(key)?;
                w.put_u8(*kind)?;
                w.put_u8(*flags)?;
                w.put_u8(*path_len)?;
                w.put(path)?;
                w.put(name)?;
                w.put_u32_le(*last_advert)?;
                if let Some((lat, lon)) = lat_lon {
                    w.put_i32_le(*lat)?;
                    w.put_i32_le(*lon)?;
                }
                if let Some(m) = lastmod {
                    w.put_u32_le(*m)?;
                }
            }
            Command::SetRadioParams {
                freq_khz,
                bw_hz,
                sf,
                cr,
                repeat,
            } => {
                w.put_u32_le(*freq_khz)?;
                w.put_u32_le(*bw_hz)?;
                w.put_u8(*sf)?;
                w.put_u8(*cr)?;
                if let Some(r) = repeat {
                    w.put_u8(*r)?;
                }
            }
            Command::SetRadioTxPower { dbm } => w.put(&dbm.to_le_bytes())?,
            Command::ResetPath { key }
            | Command::RemoveContact { key }
            | Command::ShareContact { key }
            | Command::SendStatusReq { key }
            | Command::HasConnection { key }
            | Command::Logout { key }
            | Command::GetContactByKey { key } => w.put(key)?,
            Command::SetAdvertLatLon { lat, lon, alt } => {
                w.put_i32_le(*lat)?;
                w.put_i32_le(*lon)?;
                if let Some(a) = alt {
                    w.put_i32_le(*a)?;
                }
            }
            Command::ExportContact { key } => {
                if let Some(k) = key {
                    w.put(k)?;
                }
            }
            Command::ImportContact { raw } => w.put(raw)?,
            Command::Reboot => w.put(b"reboot")?,
            Command::SetTuningParams {
                rx_delay,
                airtime_factor,
            } => {
                w.put_u32_le(*rx_delay)?;
                w.put_u32_le(*airtime_factor)?;
            }
            Command::DeviceQuery { app_ver } => w.put_u8(*app_ver)?,
            Command::ImportPrivateKey { key } => w.put(key)?,
            Command::SendRawData {
                path_len,
                path,
                payload,
            } => {
                w.put_u8(*path_len)?;
                w.put(path)?;
                w.put(payload)?;
            }
            Command::SendLogin { key, password } => {
                w.put(key)?;
                w.put(password)?;
            }
            Command::GetChannel { idx } => w.put_u8(*idx)?,
            Command::SetChannel { idx, name, secret } => {
                w.put_u8(*idx)?;
                w.put(name)?;
                w.put(secret)?;
            }
            Command::SignData { data } | Command::SendControlData { data } => w.put(data)?,
            Command::SendTracePath {
                tag,
                auth,
                flags,
                path,
            } => {
                w.put_u32_le(*tag)?;
                w.put_u32_le(*auth)?;
                w.put_u8(*flags)?;
                w.put(path)?;
            }
            Command::SetDevicePin { pin } => w.put_u32_le(*pin)?,
            Command::SetOtherParams {
                manual_add,
                telemetry,
                advert_loc_policy,
                multi_acks,
            } => {
                w.put_u8(*manual_add)?;
                for b in [telemetry, advert_loc_policy, multi_acks]
                    .into_iter()
                    .flatten()
                {
                    w.put_u8(*b)?;
                }
            }
            Command::SendTelemetryReq { reserved, key } => {
                w.put(reserved)?;
                if let Some(k) = key {
                    w.put(k)?;
                }
            }
            Command::SetCustomVar { var } => w.put(var)?,
            Command::GetAdvertPath { reserved, key } => {
                w.put_u8(*reserved)?;
                w.put(key)?;
            }
            Command::SendBinaryReq { key, data } | Command::SendAnonReq { key, data } => {
                w.put(key)?;
                w.put(data)?;
            }
            Command::FactoryReset => w.put(b"reset")?,
            Command::SendPathDiscoveryReq { key } => {
                w.put_u8(0)?;
                w.put(key)?;
            }
            Command::SetFloodScopeKey { scope } => match scope {
                FloodScopeKey::Clear => w.put_u8(0)?,
                FloodScopeKey::Key(k) => {
                    w.put_u8(0)?;
                    w.put(k)?;
                }
                FloodScopeKey::Unscoped => w.put_u8(1)?,
            },
            Command::GetStats { kind } => w.put_u8(*kind)?,
            Command::SetAutoaddConfig { config, max_hops } => {
                w.put_u8(*config)?;
                if let Some(m) = max_hops {
                    w.put_u8(*m)?;
                }
            }
            Command::SetPathHashMode { mode } => {
                w.put_u8(0)?;
                w.put_u8(*mode)?;
            }
            Command::SendChannelData {
                idx,
                path,
                data_type,
                data,
            } => {
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
            Command::SetDefaultFloodScope { scope } => {
                if let Some((name, key)) = scope {
                    w.put(name)?;
                    w.put(key)?;
                }
            }
            Command::SendRawPacket { priority, raw } => {
                w.put_u8(*priority)?;
                w.put(raw)?;
            }
            Command::RunCliCommand { cmd } => w.put(cmd)?,
            Command::GetDeviceTime
            | Command::SyncNextMessage
            | Command::GetBattAndStorage
            | Command::ExportPrivateKey
            | Command::SignStart
            | Command::SignFinish
            | Command::GetCustomVars
            | Command::GetTuningParams
            | Command::GetAutoaddConfig
            | Command::GetAllowedRepeatFreq
            | Command::GetDefaultFloodScope => {}
        }
        Ok(w.position())
    }

    /// Code de la commande.
    pub const fn code(&self) -> u8 {
        match self {
            Command::AppStart { .. } => cmd::APP_START,
            Command::SendTxtMsg { .. } => cmd::SEND_TXT_MSG,
            Command::SendChannelTxtMsg { .. } => cmd::SEND_CHANNEL_TXT_MSG,
            Command::GetContacts { .. } => cmd::GET_CONTACTS,
            Command::GetDeviceTime => cmd::GET_DEVICE_TIME,
            Command::SetDeviceTime { .. } => cmd::SET_DEVICE_TIME,
            Command::SendSelfAdvert { .. } => cmd::SEND_SELF_ADVERT,
            Command::SetAdvertName { .. } => cmd::SET_ADVERT_NAME,
            Command::AddUpdateContact { .. } => cmd::ADD_UPDATE_CONTACT,
            Command::SyncNextMessage => cmd::SYNC_NEXT_MESSAGE,
            Command::SetRadioParams { .. } => cmd::SET_RADIO_PARAMS,
            Command::SetRadioTxPower { .. } => cmd::SET_RADIO_TX_POWER,
            Command::ResetPath { .. } => cmd::RESET_PATH,
            Command::SetAdvertLatLon { .. } => cmd::SET_ADVERT_LATLON,
            Command::RemoveContact { .. } => cmd::REMOVE_CONTACT,
            Command::ShareContact { .. } => cmd::SHARE_CONTACT,
            Command::ExportContact { .. } => cmd::EXPORT_CONTACT,
            Command::ImportContact { .. } => cmd::IMPORT_CONTACT,
            Command::Reboot => cmd::REBOOT,
            Command::GetBattAndStorage => cmd::GET_BATT_AND_STORAGE,
            Command::SetTuningParams { .. } => cmd::SET_TUNING_PARAMS,
            Command::DeviceQuery { .. } => cmd::DEVICE_QUERY,
            Command::ExportPrivateKey => cmd::EXPORT_PRIVATE_KEY,
            Command::ImportPrivateKey { .. } => cmd::IMPORT_PRIVATE_KEY,
            Command::SendRawData { .. } => cmd::SEND_RAW_DATA,
            Command::SendLogin { .. } => cmd::SEND_LOGIN,
            Command::SendStatusReq { .. } => cmd::SEND_STATUS_REQ,
            Command::HasConnection { .. } => cmd::HAS_CONNECTION,
            Command::Logout { .. } => cmd::LOGOUT,
            Command::GetContactByKey { .. } => cmd::GET_CONTACT_BY_KEY,
            Command::GetChannel { .. } => cmd::GET_CHANNEL,
            Command::SetChannel { .. } => cmd::SET_CHANNEL,
            Command::SignStart => cmd::SIGN_START,
            Command::SignData { .. } => cmd::SIGN_DATA,
            Command::SignFinish => cmd::SIGN_FINISH,
            Command::SendTracePath { .. } => cmd::SEND_TRACE_PATH,
            Command::SetDevicePin { .. } => cmd::SET_DEVICE_PIN,
            Command::SetOtherParams { .. } => cmd::SET_OTHER_PARAMS,
            Command::SendTelemetryReq { .. } => cmd::SEND_TELEMETRY_REQ,
            Command::GetCustomVars => cmd::GET_CUSTOM_VARS,
            Command::SetCustomVar { .. } => cmd::SET_CUSTOM_VAR,
            Command::GetAdvertPath { .. } => cmd::GET_ADVERT_PATH,
            Command::GetTuningParams => cmd::GET_TUNING_PARAMS,
            Command::SendBinaryReq { .. } => cmd::SEND_BINARY_REQ,
            Command::FactoryReset => cmd::FACTORY_RESET,
            Command::SendPathDiscoveryReq { .. } => cmd::SEND_PATH_DISCOVERY_REQ,
            Command::SetFloodScopeKey { .. } => cmd::SET_FLOOD_SCOPE_KEY,
            Command::SendControlData { .. } => cmd::SEND_CONTROL_DATA,
            Command::GetStats { .. } => cmd::GET_STATS,
            Command::SendAnonReq { .. } => cmd::SEND_ANON_REQ,
            Command::SetAutoaddConfig { .. } => cmd::SET_AUTOADD_CONFIG,
            Command::GetAutoaddConfig => cmd::GET_AUTOADD_CONFIG,
            Command::GetAllowedRepeatFreq => cmd::GET_ALLOWED_REPEAT_FREQ,
            Command::SetPathHashMode { .. } => cmd::SET_PATH_HASH_MODE,
            Command::SendChannelData { .. } => cmd::SEND_CHANNEL_DATA,
            Command::SetDefaultFloodScope { .. } => cmd::SET_DEFAULT_FLOOD_SCOPE,
            Command::GetDefaultFloodScope => cmd::GET_DEFAULT_FLOOD_SCOPE,
            Command::SendRawPacket { .. } => cmd::SEND_RAW_PACKET,
            Command::RunCliCommand { .. } => cmd::RUN_CLI_COMMAND,
        }
    }

    /// Décodage **strict** (app, outils, tests) : canonique, octets en trop refusés.
    pub fn decode(frame: &'a [u8]) -> Result<Self, DecodeError> {
        if frame.len() > FRAME_MAX {
            return Err(DecodeError::Length);
        }
        let mut r = Reader::new(frame);
        let c = match r.u8()? {
            cmd::APP_START => Command::AppStart {
                reserved: *r.array()?,
                app_name: r.rest(),
            },
            cmd::SEND_TXT_MSG => Command::SendTxtMsg {
                txt_type: r.u8()?,
                attempt: r.u8()?,
                timestamp: r.u32_le()?,
                prefix: *r.array()?,
                text: rest_min(&mut r, 1)?,
            },
            cmd::SEND_CHANNEL_TXT_MSG => Command::SendChannelTxtMsg {
                txt_type: r.u8()?,
                idx: r.u8()?,
                timestamp: r.u32_le()?,
                text: r.rest(),
            },
            cmd::GET_CONTACTS => Command::GetContacts {
                since: opt_u32(&mut r)?,
            },
            cmd::GET_DEVICE_TIME => Command::GetDeviceTime,
            cmd::SET_DEVICE_TIME => Command::SetDeviceTime {
                unix_s: r.u32_le()?,
            },
            cmd::SEND_SELF_ADVERT => Command::SendSelfAdvert {
                flood: match opt_u8(&mut r)? {
                    None => None,
                    Some(0) => Some(false),
                    Some(1) => Some(true),
                    Some(_) => return Err(DecodeError::Value),
                },
            },
            cmd::SET_ADVERT_NAME => Command::SetAdvertName {
                name: rest_min(&mut r, 1)?,
            },
            cmd::ADD_UPDATE_CONTACT => {
                let key = *r.array()?;
                let kind = r.u8()?;
                let flags = r.u8()?;
                let path_len = r.u8()?;
                let path = *r.array()?;
                let name = *r.array()?;
                let last_advert = r.u32_le()?;
                let lat_lon = match opt_i32(&mut r)? {
                    None => None,
                    Some(lat) => Some((lat, r.i32_le()?)),
                };
                let lastmod = if lat_lon.is_some() {
                    opt_u32(&mut r)?
                } else {
                    None
                };
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
                }
            }
            cmd::SYNC_NEXT_MESSAGE => Command::SyncNextMessage,
            cmd::SET_RADIO_PARAMS => Command::SetRadioParams {
                freq_khz: r.u32_le()?,
                bw_hz: r.u32_le()?,
                sf: r.u8()?,
                cr: r.u8()?,
                repeat: opt_u8(&mut r)?,
            },
            cmd::SET_RADIO_TX_POWER => Command::SetRadioTxPower {
                dbm: i8_of(r.u8()?),
            },
            cmd::RESET_PATH => Command::ResetPath { key: *r.array()? },
            cmd::SET_ADVERT_LATLON => Command::SetAdvertLatLon {
                lat: r.i32_le()?,
                lon: r.i32_le()?,
                alt: opt_i32(&mut r)?,
            },
            cmd::REMOVE_CONTACT => Command::RemoveContact { key: *r.array()? },
            cmd::SHARE_CONTACT => Command::ShareContact { key: *r.array()? },
            cmd::EXPORT_CONTACT => Command::ExportContact {
                key: opt_key(&mut r)?,
            },
            cmd::IMPORT_CONTACT => Command::ImportContact {
                raw: rest_min(&mut r, 98)?,
            },
            cmd::REBOOT => {
                if r.array::<6>()? != b"reboot" {
                    return Err(DecodeError::Value);
                }
                Command::Reboot
            }
            cmd::GET_BATT_AND_STORAGE => Command::GetBattAndStorage,
            cmd::SET_TUNING_PARAMS => Command::SetTuningParams {
                rx_delay: r.u32_le()?,
                airtime_factor: r.u32_le()?,
            },
            cmd::DEVICE_QUERY => Command::DeviceQuery { app_ver: r.u8()? },
            cmd::EXPORT_PRIVATE_KEY => Command::ExportPrivateKey,
            cmd::IMPORT_PRIVATE_KEY => Command::ImportPrivateKey { key: *r.array()? },
            cmd::SEND_RAW_DATA => {
                let (path_len, path) = read_path(&mut r)?;
                Command::SendRawData {
                    path_len,
                    path,
                    payload: rest_min(&mut r, 4)?,
                }
            }
            cmd::SEND_LOGIN => Command::SendLogin {
                key: *r.array()?,
                password: r.rest(),
            },
            cmd::SEND_STATUS_REQ => Command::SendStatusReq { key: *r.array()? },
            cmd::HAS_CONNECTION => Command::HasConnection { key: *r.array()? },
            cmd::LOGOUT => Command::Logout { key: *r.array()? },
            cmd::GET_CONTACT_BY_KEY => Command::GetContactByKey { key: *r.array()? },
            cmd::GET_CHANNEL => Command::GetChannel { idx: r.u8()? },
            cmd::SET_CHANNEL => Command::SetChannel {
                idx: r.u8()?,
                name: *r.array()?,
                secret: *r.array()?,
            },
            cmd::SIGN_START => Command::SignStart,
            cmd::SIGN_DATA => Command::SignData {
                data: rest_min(&mut r, 1)?,
            },
            cmd::SIGN_FINISH => Command::SignFinish,
            cmd::SEND_TRACE_PATH => {
                let tag = r.u32_le()?;
                let auth = r.u32_le()?;
                let flags = r.u8()?;
                let path = rest_min(&mut r, 1)?;
                if !trace_path_ok(flags, path.len()) {
                    return Err(DecodeError::Value);
                }
                Command::SendTracePath {
                    tag,
                    auth,
                    flags,
                    path,
                }
            }
            cmd::SET_DEVICE_PIN => Command::SetDevicePin { pin: r.u32_le()? },
            cmd::SET_OTHER_PARAMS => {
                let manual_add = r.u8()?;
                let telemetry = opt_u8(&mut r)?;
                let advert_loc_policy = if telemetry.is_some() {
                    opt_u8(&mut r)?
                } else {
                    None
                };
                let multi_acks = if advert_loc_policy.is_some() {
                    opt_u8(&mut r)?
                } else {
                    None
                };
                Command::SetOtherParams {
                    manual_add,
                    telemetry,
                    advert_loc_policy,
                    multi_acks,
                }
            }
            cmd::SEND_TELEMETRY_REQ => Command::SendTelemetryReq {
                reserved: *r.array()?,
                key: opt_key(&mut r)?,
            },
            cmd::GET_CUSTOM_VARS => Command::GetCustomVars,
            cmd::SET_CUSTOM_VAR => Command::SetCustomVar {
                var: rest_min(&mut r, 3)?,
            },
            cmd::GET_ADVERT_PATH => Command::GetAdvertPath {
                reserved: r.u8()?,
                key: *r.array()?,
            },
            cmd::GET_TUNING_PARAMS => Command::GetTuningParams,
            cmd::SEND_BINARY_REQ => Command::SendBinaryReq {
                key: *r.array()?,
                data: rest_min(&mut r, 1)?,
            },
            cmd::FACTORY_RESET => {
                if r.array::<5>()? != b"reset" {
                    return Err(DecodeError::Value);
                }
                Command::FactoryReset
            }
            cmd::SEND_PATH_DISCOVERY_REQ => {
                if r.u8()? != 0 {
                    return Err(DecodeError::Value);
                }
                Command::SendPathDiscoveryReq { key: *r.array()? }
            }
            cmd::SET_FLOOD_SCOPE_KEY => Command::SetFloodScopeKey {
                scope: match r.u8()? {
                    0 if r.is_empty() => FloodScopeKey::Clear,
                    0 => FloodScopeKey::Key(*r.array()?),
                    1 => FloodScopeKey::Unscoped,
                    _ => return Err(DecodeError::Value),
                },
            },
            cmd::SEND_CONTROL_DATA => {
                let data = rest_min(&mut r, 1)?;
                if data.first().is_none_or(|b| b & 0x80 == 0) {
                    return Err(DecodeError::Value);
                }
                Command::SendControlData { data }
            }
            cmd::GET_STATS => Command::GetStats { kind: r.u8()? },
            cmd::SEND_ANON_REQ => Command::SendAnonReq {
                key: *r.array()?,
                data: rest_min(&mut r, 1)?,
            },
            cmd::SET_AUTOADD_CONFIG => Command::SetAutoaddConfig {
                config: r.u8()?,
                max_hops: opt_u8(&mut r)?,
            },
            cmd::GET_AUTOADD_CONFIG => Command::GetAutoaddConfig,
            cmd::GET_ALLOWED_REPEAT_FREQ => Command::GetAllowedRepeatFreq,
            cmd::SET_PATH_HASH_MODE => {
                if r.u8()? != 0 {
                    return Err(DecodeError::Value);
                }
                Command::SetPathHashMode { mode: r.u8()? }
            }
            cmd::SEND_CHANNEL_DATA => {
                let idx = r.u8()?;
                let path = if r.peek_rest().first() == Some(&PATH_UNKNOWN) {
                    r.u8()?;
                    None
                } else {
                    Some(read_path(&mut r)?)
                };
                Command::SendChannelData {
                    idx,
                    path,
                    data_type: r.u16_le()?,
                    data: r.rest(),
                }
            }
            cmd::SET_DEFAULT_FLOOD_SCOPE => Command::SetDefaultFloodScope {
                scope: if r.is_empty() {
                    None
                } else {
                    Some((*r.array()?, *r.array()?))
                },
            },
            cmd::GET_DEFAULT_FLOOD_SCOPE => Command::GetDefaultFloodScope,
            cmd::SEND_RAW_PACKET => Command::SendRawPacket {
                priority: r.u8()?,
                // Au moins un en-tête et un path_len (le C++ exige len ≥ 4, M:2045).
                raw: rest_min(&mut r, 2)?,
            },
            cmd::RUN_CLI_COMMAND => Command::RunCliCommand {
                cmd: rest_min(&mut r, 2)?,
            },
            _ => return Err(DecodeError::Code),
        };
        end(&r)?;
        Ok(c)
    }

    /// Décodage **côté appareil** : accepte exactement ce qu'accepte `MyMesh::handleCmdFrame`
    /// (M:1055-2062) au stade de l'analyse, et refuse avec l'erreur qu'il répondrait.
    ///
    /// Octets en trop ignorés là où le C++ les ignore ; champs finaux optionnels selon la
    /// longueur ; octets lus au-delà de la trame (le C++ ne vérifie pas toujours la longueur)
    /// valant 0. Le détail, branche par branche, est dans `docs/spec/companion.md` §10.
    pub fn decode_device(frame: &'a [u8]) -> Result<Self, DeviceReject> {
        let f = frame;
        let len = f.len();
        let Some(&code) = f.first() else {
            return Err(DeviceReject::Ignored);
        };
        if len > FRAME_MAX {
            return Err(DeviceReject::Ignored);
        }
        let c = match code {
            // M:1056
            cmd::DEVICE_QUERY if len >= 2 => Command::DeviceQuery {
                app_ver: byte(f, 1),
            },
            // M:1076
            cmd::APP_START if len >= 8 => Command::AppStart {
                reserved: pad(f, 1),
                app_name: tail(f, 8),
            },
            // M:1118
            cmd::RUN_CLI_COMMAND if len >= 3 => Command::RunCliCommand { cmd: tail(f, 1) },
            // M:1132
            cmd::SEND_TXT_MSG if len >= 14 => Command::SendTxtMsg {
                txt_type: byte(f, 1),
                attempt: byte(f, 2),
                timestamp: u32_at(f, 3),
                prefix: pad(f, 7),
                text: tail(f, 13),
            },
            // M:1178, sans contrôle de longueur. En dessous de 7 octets, le C++ passe une longueur
            // de texte négative à memcpy (BCM:528) : comportement indéfini, refusé ici (§10).
            cmd::SEND_CHANNEL_TXT_MSG if len >= 7 => Command::SendChannelTxtMsg {
                txt_type: byte(f, 1),
                idx: byte(f, 2),
                timestamp: u32_at(f, 3),
                text: tail(f, 7),
            },
            // M:1198
            cmd::SEND_CHANNEL_DATA => return device_channel_data(f),
            // M:1238 : since seulement si len ≥ 5, sinon 0.
            cmd::GET_CONTACTS => Command::GetContacts {
                since: (len >= 5).then(|| u32_at(f, 1)),
            },
            // M:1259
            cmd::SET_ADVERT_NAME if len >= 2 => Command::SetAdvertName { name: tail(f, 1) },
            // M:1266 : alt si len ≥ 13.
            cmd::SET_ADVERT_LATLON if len >= 9 => Command::SetAdvertLatLon {
                lat: i32_at(f, 1),
                lon: i32_at(f, 5),
                alt: (len >= 13).then(|| i32_at(f, 9)),
            },
            // M:1281
            cmd::GET_DEVICE_TIME => Command::GetDeviceTime,
            // M:1287
            cmd::SET_DEVICE_TIME if len >= 5 => Command::SetDeviceTime {
                unix_s: u32_at(f, 1),
            },
            // M:1297 : inondation seulement si [1] == 1.
            cmd::SEND_SELF_ADVERT => Command::SendSelfAdvert {
                flood: (len >= 2).then(|| byte(f, 1) == 1),
            },
            // M:1317
            cmd::RESET_PATH if len >= 33 => Command::ResetPath { key: pad(f, 1) },
            // M:1328, champs lus jusqu'à [135] (M:194-217), lat/lon si len ≥ 144, lastmod si
            // len ≥ 148.
            cmd::ADD_UPDATE_CONTACT if len >= 36 => Command::AddUpdateContact {
                key: pad(f, 1),
                kind: byte(f, 33),
                flags: byte(f, 34),
                path_len: byte(f, 35),
                path: pad(f, 36),
                name: pad(f, 100),
                last_advert: u32_at(f, 132),
                lat_lon: (len >= 144).then(|| (i32_at(f, 136), i32_at(f, 140))),
                lastmod: (len >= 148).then(|| u32_at(f, 144)),
            },
            // M:1349, M:1359, M:1371 : clé lue sans contrôle de longueur.
            cmd::REMOVE_CONTACT => Command::RemoveContact { key: pad(f, 1) },
            cmd::SHARE_CONTACT => Command::ShareContact { key: pad(f, 1) },
            cmd::GET_CONTACT_BY_KEY => Command::GetContactByKey { key: pad(f, 1) },
            // M:1379 : len < 33 → soi-même.
            cmd::EXPORT_CONTACT => Command::ExportContact {
                key: (len >= 33).then(|| pad(f, 1)),
            },
            // M:1409
            cmd::IMPORT_CONTACT if len > 98 => Command::ImportContact { raw: tail(f, 1) },
            // M:1415
            cmd::SYNC_NEXT_MESSAGE => Command::SyncNextMessage,
            // M:1424, sans contrôle de longueur ; repeat si len > 11.
            cmd::SET_RADIO_PARAMS => Command::SetRadioParams {
                freq_khz: u32_at(f, 1),
                bw_hz: u32_at(f, 5),
                sf: byte(f, 9),
                cr: byte(f, 10),
                repeat: (len > 11).then(|| byte(f, 11)),
            },
            // M:1460
            cmd::SET_RADIO_TX_POWER => Command::SetRadioTxPower {
                dbm: i8_of(byte(f, 1)),
            },
            // M:1470
            cmd::SET_TUNING_PARAMS => Command::SetTuningParams {
                rx_delay: u32_at(f, 1),
                airtime_factor: u32_at(f, 5),
            },
            // M:1481
            cmd::GET_TUNING_PARAMS => Command::GetTuningParams,
            // M:1488 : [1] toujours lu ; [2], [3], [4] si len ≥ 3, 4, 5.
            cmd::SET_OTHER_PARAMS => Command::SetOtherParams {
                manual_add: byte(f, 1),
                telemetry: (len >= 3).then(|| byte(f, 2)),
                advert_loc_policy: (len >= 4).then(|| byte(f, 3)),
                multi_acks: (len >= 5).then(|| byte(f, 4)),
            },
            // M:1504
            cmd::SET_PATH_HASH_MODE if byte(f, 1) == 0 && len >= 3 => {
                Command::SetPathHashMode { mode: byte(f, 2) }
            }
            // M:1512 : memcmp sur 6 octets, quelle que soit la longueur.
            cmd::REBOOT if pad::<6>(f, 1) == *b"reboot" => Command::Reboot,
            // M:1517, M:1528
            cmd::GET_BATT_AND_STORAGE => Command::GetBattAndStorage,
            cmd::EXPORT_PRIVATE_KEY => Command::ExportPrivateKey,
            // M:1537
            cmd::IMPORT_PRIVATE_KEY if len >= 65 => Command::ImportPrivateKey { key: pad(f, 1) },
            // M:1557
            cmd::SEND_RAW_DATA if len >= 6 => return device_raw_data(f),
            // M:1577
            cmd::SEND_LOGIN if len >= 33 => Command::SendLogin {
                key: pad(f, 1),
                password: tail(f, 33),
            },
            // M:1599
            cmd::SEND_ANON_REQ if len > 33 => Command::SendAnonReq {
                key: pad(f, 1),
                data: tail(f, 33),
            },
            // M:1630
            cmd::SEND_STATUS_REQ if len >= 33 => Command::SendStatusReq { key: pad(f, 1) },
            // M:1651
            cmd::SEND_PATH_DISCOVERY_REQ if byte(f, 1) == 0 && len >= 34 => {
                Command::SendPathDiscoveryReq { key: pad(f, 2) }
            }
            // M:1680 puis M:1700.
            cmd::SEND_TELEMETRY_REQ if len >= 36 => Command::SendTelemetryReq {
                reserved: pad(f, 1),
                key: Some(pad(f, 4)),
            },
            cmd::SEND_TELEMETRY_REQ if len == 4 => Command::SendTelemetryReq {
                reserved: pad(f, 1),
                key: None,
            },
            // M:1720
            cmd::SEND_BINARY_REQ if len >= 34 => Command::SendBinaryReq {
                key: pad(f, 1),
                data: tail(f, 33),
            },
            // M:1741, M:1748
            cmd::HAS_CONNECTION if len >= 33 => Command::HasConnection { key: pad(f, 1) },
            cmd::LOGOUT if len >= 33 => Command::Logout { key: pad(f, 1) },
            // M:1752
            cmd::GET_CHANNEL if len >= 2 => Command::GetChannel { idx: byte(f, 1) },
            // M:1767 : une clé de 32 octets (len ≥ 66) n'est pas prise en charge.
            cmd::SET_CHANNEL if len >= 66 => return Err(DeviceReject::UNSUPPORTED),
            // M:1769
            cmd::SET_CHANNEL if len >= 50 => Command::SetChannel {
                idx: byte(f, 1),
                name: pad(f, 2),
                secret: pad(f, 34),
            },
            // M:1781, M:1793, M:1801
            cmd::SIGN_START => Command::SignStart,
            cmd::SIGN_DATA if len > 1 => Command::SignData { data: tail(f, 1) },
            cmd::SIGN_FINISH => Command::SignFinish,
            // M:1813 (len − 10 < 179 est toujours vrai)
            cmd::SEND_TRACE_PATH if len > 10 => return device_trace(f),
            // M:1839
            cmd::SET_DEVICE_PIN if len >= 5 => Command::SetDevicePin { pin: u32_at(f, 1) },
            // M:1853, M:1867
            cmd::GET_CUSTOM_VARS => Command::GetCustomVars,
            cmd::SET_CUSTOM_VAR if len >= 4 => Command::SetCustomVar { var: tail(f, 1) },
            // M:1893
            cmd::GET_ADVERT_PATH if len >= 34 => Command::GetAdvertPath {
                reserved: byte(f, 1),
                key: pad(f, 2),
            },
            // M:1914 (type inconnu → ERR 6, après l'analyse)
            cmd::GET_STATS if len >= 2 => Command::GetStats { kind: byte(f, 1) },
            // M:1965 : memcmp sur 5 octets, quelle que soit la longueur.
            cmd::FACTORY_RESET if pad::<5>(f, 1) == *b"reset" => Command::FactoryReset,
            // M:1978 : clé si len ≥ 18, sinon effacement.
            cmd::SET_FLOOD_SCOPE_KEY if len >= 2 && byte(f, 1) == 0 => Command::SetFloodScopeKey {
                scope: if len >= 18 {
                    FloodScopeKey::Key(pad(f, 2))
                } else {
                    FloodScopeKey::Clear
                },
            },
            // M:1986
            cmd::SET_FLOOD_SCOPE_KEY if len >= 2 && byte(f, 1) == 1 => Command::SetFloodScopeKey {
                scope: FloodScopeKey::Unscoped,
            },
            // M:1989 : portée si len ≥ 48, sinon effacement.
            cmd::SET_DEFAULT_FLOOD_SCOPE => Command::SetDefaultFloodScope {
                scope: (len >= 48).then(|| (pad(f, 1), pad(f, 32))),
            },
            // M:2006
            cmd::GET_DEFAULT_FLOOD_SCOPE => Command::GetDefaultFloodScope,
            // M:2015
            cmd::SEND_CONTROL_DATA if len >= 2 && byte(f, 1) & 0x80 != 0 => {
                Command::SendControlData { data: tail(f, 1) }
            }
            // M:2023 : [1] toujours lu ; max_hops si len ≥ 3.
            cmd::SET_AUTOADD_CONFIG => Command::SetAutoaddConfig {
                config: byte(f, 1),
                max_hops: (len >= 3).then(|| byte(f, 2)),
            },
            // M:2030, M:2036
            cmd::GET_AUTOADD_CONFIG => Command::GetAutoaddConfig,
            cmd::GET_ALLOWED_REPEAT_FREQ => Command::GetAllowedRepeatFreq,
            // M:2045
            cmd::SEND_RAW_PACKET if len >= 4 => Command::SendRawPacket {
                priority: byte(f, 1),
                raw: tail(f, 2),
            },
            // M:2059 : code inconnu ou précondition ratée.
            _ => return Err(DeviceReject::UNSUPPORTED),
        };
        Ok(c)
    }
}

// ------------------------------------------------------------------------------------ affichage

fn key_or_self(f: &mut fmt::Formatter<'_>, key: &Option<[u8; 32]>) -> fmt::Result {
    match key {
        Some(k) => write!(f, " key={}", Hex(k)),
        None => f.write_str(" self"),
    }
}

impl fmt::Display for FloodScopeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FloodScopeKey::Clear => f.write_str("clear"),
            FloodScopeKey::Key(k) => write!(f, "key={}", Hex(k)),
            FloodScopeKey::Unscoped => f.write_str("unscoped"),
        }
    }
}

impl fmt::Display for Command<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Command::AppStart { reserved, app_name } => write!(
                f,
                "APP_START reserved={} app={}",
                Hex(reserved),
                Text(app_name)
            ),
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
            Command::SendChannelTxtMsg {
                txt_type,
                idx,
                timestamp,
                text,
            } => write!(
                f,
                "SEND_CHANNEL_TXT_MSG {idx} type={txt_type} ts={timestamp} text={}",
                Text(text)
            ),
            Command::GetContacts { since: None } => f.write_str("GET_CONTACTS"),
            Command::GetContacts { since: Some(s) } => write!(f, "GET_CONTACTS since={s}"),
            Command::GetDeviceTime => f.write_str("GET_DEVICE_TIME"),
            Command::SetDeviceTime { unix_s } => write!(f, "SET_DEVICE_TIME {unix_s}"),
            Command::SendSelfAdvert { flood: None } => f.write_str("SEND_SELF_ADVERT"),
            Command::SendSelfAdvert { flood: Some(b) } => write!(f, "SEND_SELF_ADVERT flood={b}"),
            Command::SetAdvertName { name } => write!(f, "SET_ADVERT_NAME {}", Text(name)),
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
                write!(
                    f,
                    "ADD_UPDATE_CONTACT key={} type={kind} flags={flags} path_len={path_len} path={} name={} last_advert={last_advert}",
                    Hex(key),
                    Hex(path),
                    Text(fixed_str(name))
                )?;
                if let Some((lat, lon)) = lat_lon {
                    write!(f, " lat={lat} lon={lon}")?;
                }
                if let Some(m) = lastmod {
                    write!(f, " lastmod={m}")?;
                }
                Ok(())
            }
            Command::SyncNextMessage => f.write_str("SYNC_NEXT_MESSAGE"),
            Command::SetRadioParams {
                freq_khz,
                bw_hz,
                sf,
                cr,
                repeat,
            } => {
                write!(
                    f,
                    "SET_RADIO_PARAMS freq={freq_khz}kHz bw={bw_hz}Hz sf={sf} cr={cr}"
                )?;
                if let Some(r) = repeat {
                    write!(f, " repeat={r}")?;
                }
                Ok(())
            }
            Command::SetRadioTxPower { dbm } => write!(f, "SET_RADIO_TX_POWER {dbm}dBm"),
            Command::ResetPath { key } => write!(f, "RESET_PATH key={}", Hex(key)),
            Command::SetAdvertLatLon { lat, lon, alt } => {
                write!(f, "SET_ADVERT_LATLON lat={lat} lon={lon}")?;
                if let Some(a) = alt {
                    write!(f, " alt={a}")?;
                }
                Ok(())
            }
            Command::RemoveContact { key } => write!(f, "REMOVE_CONTACT key={}", Hex(key)),
            Command::ShareContact { key } => write!(f, "SHARE_CONTACT key={}", Hex(key)),
            Command::ExportContact { key } => {
                f.write_str("EXPORT_CONTACT")?;
                key_or_self(f, key)
            }
            Command::ImportContact { raw } => write!(f, "IMPORT_CONTACT raw={}", Hex(raw)),
            Command::Reboot => f.write_str("REBOOT"),
            Command::GetBattAndStorage => f.write_str("GET_BATT_AND_STORAGE"),
            Command::SetTuningParams {
                rx_delay,
                airtime_factor,
            } => write!(
                f,
                "SET_TUNING_PARAMS rx_delay={rx_delay}/1000 airtime_factor={airtime_factor}/1000"
            ),
            Command::DeviceQuery { app_ver } => write!(f, "DEVICE_QUERY app_ver={app_ver}"),
            Command::ExportPrivateKey => f.write_str("EXPORT_PRIVATE_KEY"),
            Command::ImportPrivateKey { .. } => f.write_str("IMPORT_PRIVATE_KEY <64 octets>"),
            Command::SendRawData {
                path_len,
                path,
                payload,
            } => write!(
                f,
                "SEND_RAW_DATA path={path_len:02x}:{} payload={}",
                Hex(path),
                Hex(payload)
            ),
            Command::SendLogin { key, password } => write!(
                f,
                "SEND_LOGIN key={} password=<{} octets>",
                Hex(key),
                password.len()
            ),
            Command::SendStatusReq { key } => write!(f, "SEND_STATUS_REQ key={}", Hex(key)),
            Command::HasConnection { key } => write!(f, "HAS_CONNECTION key={}", Hex(key)),
            Command::Logout { key } => write!(f, "LOGOUT key={}", Hex(key)),
            Command::GetContactByKey { key } => {
                write!(f, "GET_CONTACT_BY_KEY key={}", Hex(key))
            }
            Command::GetChannel { idx } => write!(f, "GET_CHANNEL {idx}"),
            Command::SetChannel { idx, name, secret } => write!(
                f,
                "SET_CHANNEL {idx} name={} secret={}",
                Text(fixed_str(name)),
                Hex(secret)
            ),
            Command::SignStart => f.write_str("SIGN_START"),
            Command::SignData { data } => write!(f, "SIGN_DATA {}", Hex(data)),
            Command::SignFinish => f.write_str("SIGN_FINISH"),
            Command::SendTracePath {
                tag,
                auth,
                flags,
                path,
            } => write!(
                f,
                "SEND_TRACE_PATH tag={tag} auth={auth} flags={flags} path={}",
                Hex(path)
            ),
            Command::SetDevicePin { pin } => write!(f, "SET_DEVICE_PIN {pin}"),
            Command::SetOtherParams {
                manual_add,
                telemetry,
                advert_loc_policy,
                multi_acks,
            } => {
                write!(f, "SET_OTHER_PARAMS manual_add={manual_add}")?;
                if let Some(t) = telemetry {
                    write!(f, " telemetry={t:#04x}")?;
                }
                if let Some(p) = advert_loc_policy {
                    write!(f, " advert_loc_policy={p}")?;
                }
                if let Some(m) = multi_acks {
                    write!(f, " multi_acks={m}")?;
                }
                Ok(())
            }
            Command::SendTelemetryReq { reserved, key } => {
                write!(f, "SEND_TELEMETRY_REQ reserved={}", Hex(reserved))?;
                key_or_self(f, key)
            }
            Command::GetCustomVars => f.write_str("GET_CUSTOM_VARS"),
            Command::SetCustomVar { var } => write!(f, "SET_CUSTOM_VAR {}", Text(var)),
            Command::GetAdvertPath { reserved, key } => {
                write!(f, "GET_ADVERT_PATH reserved={reserved} key={}", Hex(key))
            }
            Command::GetTuningParams => f.write_str("GET_TUNING_PARAMS"),
            Command::SendBinaryReq { key, data } => {
                write!(f, "SEND_BINARY_REQ key={} data={}", Hex(key), Hex(data))
            }
            Command::FactoryReset => f.write_str("FACTORY_RESET"),
            Command::SendPathDiscoveryReq { key } => {
                write!(f, "SEND_PATH_DISCOVERY_REQ key={}", Hex(key))
            }
            Command::SetFloodScopeKey { scope } => write!(f, "SET_FLOOD_SCOPE_KEY {scope}"),
            Command::SendControlData { data } => write!(f, "SEND_CONTROL_DATA {}", Hex(data)),
            Command::GetStats { kind } => write!(f, "GET_STATS {kind}"),
            Command::SendAnonReq { key, data } => {
                write!(f, "SEND_ANON_REQ key={} data={}", Hex(key), Hex(data))
            }
            Command::SetAutoaddConfig { config, max_hops } => {
                write!(f, "SET_AUTOADD_CONFIG config={config:#04x}")?;
                if let Some(m) = max_hops {
                    write!(f, " max_hops={m}")?;
                }
                Ok(())
            }
            Command::GetAutoaddConfig => f.write_str("GET_AUTOADD_CONFIG"),
            Command::GetAllowedRepeatFreq => f.write_str("GET_ALLOWED_REPEAT_FREQ"),
            Command::SetPathHashMode { mode } => write!(f, "SET_PATH_HASH_MODE {mode}"),
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
            Command::GetDefaultFloodScope => f.write_str("GET_DEFAULT_FLOOD_SCOPE"),
            Command::SendRawPacket { priority, raw } => {
                write!(f, "SEND_RAW_PACKET prio={priority} raw={}", Hex(raw))
            }
            Command::RunCliCommand { cmd } => write!(f, "RUN_CLI_COMMAND {}", Text(cmd)),
        }
    }
}
