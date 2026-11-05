//! Éléments communs du protocole compagnon MeshCore (`docs/spec/companion.md`) : codes, erreurs,
//! champs de taille fixe, chemins. Les trames elles-mêmes sont dans [`crate::command`] (app →
//! appareil) et [`crate::response`] (appareil → app).
//!
//! Décodage zéro-copie. Le décodage **strict** est canonique : `encode(decode(x)) == x` pour toute
//! trame acceptée, et `encode` n'écrit que des trames que `decode` relit à l'identique. Les champs
//! de taille fixe sont gardés tels quels (fin d'un nom, octets réservés), même quand l'appareil y
//! laisse des octets sans signification. Le décodage **côté appareil** des commandes
//! ([`crate::Command::decode_device`]) reproduit les tolérances du C++ (§10, ADR-0002).

use core::fmt;
use mc_core::{Reader, Truncated};

pub use crate::command::{Command, FloodScopeKey};
pub use crate::response::{
    Contact, DeviceInfo, LoginServer, Response, SelfInfo, Stats, freq_ranges,
};

/// Taille maximale d'une trame (`MAX_FRAME_SIZE` du firmware).
pub const FRAME_MAX: usize = 176;
/// Version du protocole annoncée dans DEVICE_QUERY : messages reçus au format V3.
pub const APP_VER: u8 = 3;
/// `path_len` d'un chemin inconnu, ou d'une inondation.
pub const PATH_UNKNOWN: u8 = 0xFF;
/// Type d'un contact « compagnon » (`ADV_TYPE_CHAT`).
pub const ADV_TYPE_CHAT: u8 = 1;
/// Taille maximale d'un chemin en octets (`MAX_PATH_SIZE`, `src/MeshCore.h:22`).
pub const MAX_PATH: usize = 64;

/// Codes des commandes (app → appareil), M:9-73. 44 à 49 et 53 sont libres (ERR 1).
pub mod cmd {
    pub const APP_START: u8 = 1;
    pub const SEND_TXT_MSG: u8 = 2;
    pub const SEND_CHANNEL_TXT_MSG: u8 = 3;
    pub const GET_CONTACTS: u8 = 4;
    pub const GET_DEVICE_TIME: u8 = 5;
    pub const SET_DEVICE_TIME: u8 = 6;
    pub const SEND_SELF_ADVERT: u8 = 7;
    pub const SET_ADVERT_NAME: u8 = 8;
    pub const ADD_UPDATE_CONTACT: u8 = 9;
    pub const SYNC_NEXT_MESSAGE: u8 = 10;
    pub const SET_RADIO_PARAMS: u8 = 11;
    pub const SET_RADIO_TX_POWER: u8 = 12;
    pub const RESET_PATH: u8 = 13;
    pub const SET_ADVERT_LATLON: u8 = 14;
    pub const REMOVE_CONTACT: u8 = 15;
    pub const SHARE_CONTACT: u8 = 16;
    pub const EXPORT_CONTACT: u8 = 17;
    pub const IMPORT_CONTACT: u8 = 18;
    pub const REBOOT: u8 = 19;
    pub const GET_BATT_AND_STORAGE: u8 = 20;
    pub const SET_TUNING_PARAMS: u8 = 21;
    pub const DEVICE_QUERY: u8 = 22;
    pub const EXPORT_PRIVATE_KEY: u8 = 23;
    pub const IMPORT_PRIVATE_KEY: u8 = 24;
    pub const SEND_RAW_DATA: u8 = 25;
    pub const SEND_LOGIN: u8 = 26;
    pub const SEND_STATUS_REQ: u8 = 27;
    pub const HAS_CONNECTION: u8 = 28;
    pub const LOGOUT: u8 = 29;
    pub const GET_CONTACT_BY_KEY: u8 = 30;
    pub const GET_CHANNEL: u8 = 31;
    pub const SET_CHANNEL: u8 = 32;
    pub const SIGN_START: u8 = 33;
    pub const SIGN_DATA: u8 = 34;
    pub const SIGN_FINISH: u8 = 35;
    pub const SEND_TRACE_PATH: u8 = 36;
    pub const SET_DEVICE_PIN: u8 = 37;
    pub const SET_OTHER_PARAMS: u8 = 38;
    pub const SEND_TELEMETRY_REQ: u8 = 39;
    pub const GET_CUSTOM_VARS: u8 = 40;
    pub const SET_CUSTOM_VAR: u8 = 41;
    pub const GET_ADVERT_PATH: u8 = 42;
    pub const GET_TUNING_PARAMS: u8 = 43;
    pub const SEND_BINARY_REQ: u8 = 50;
    pub const FACTORY_RESET: u8 = 51;
    pub const SEND_PATH_DISCOVERY_REQ: u8 = 52;
    pub const SET_FLOOD_SCOPE_KEY: u8 = 54;
    pub const SEND_CONTROL_DATA: u8 = 55;
    pub const GET_STATS: u8 = 56;
    pub const SEND_ANON_REQ: u8 = 57;
    pub const SET_AUTOADD_CONFIG: u8 = 58;
    pub const GET_AUTOADD_CONFIG: u8 = 59;
    pub const GET_ALLOWED_REPEAT_FREQ: u8 = 60;
    pub const SET_PATH_HASH_MODE: u8 = 61;
    pub const SEND_CHANNEL_DATA: u8 = 62;
    pub const SET_DEFAULT_FLOOD_SCOPE: u8 = 63;
    pub const GET_DEFAULT_FLOOD_SCOPE: u8 = 64;
    pub const SEND_RAW_PACKET: u8 = 65;
    pub const RUN_CLI_COMMAND: u8 = 66;
}

/// Codes des réponses (0..29) et des notifications (0x80..0x90), M:76-134.
pub mod resp {
    pub const OK: u8 = 0;
    pub const ERR: u8 = 1;
    pub const CONTACTS_START: u8 = 2;
    pub const CONTACT: u8 = 3;
    pub const END_OF_CONTACTS: u8 = 4;
    pub const SELF_INFO: u8 = 5;
    pub const SENT: u8 = 6;
    pub const CONTACT_MSG_RECV: u8 = 7;
    pub const CHANNEL_MSG_RECV: u8 = 8;
    pub const CURR_TIME: u8 = 9;
    pub const NO_MORE_MESSAGES: u8 = 10;
    pub const EXPORT_CONTACT: u8 = 11;
    pub const BATT_AND_STORAGE: u8 = 12;
    pub const DEVICE_INFO: u8 = 13;
    pub const PRIVATE_KEY: u8 = 14;
    pub const DISABLED: u8 = 15;
    pub const CONTACT_MSG_RECV_V3: u8 = 16;
    pub const CHANNEL_MSG_RECV_V3: u8 = 17;
    pub const CHANNEL_INFO: u8 = 18;
    pub const SIGN_START: u8 = 19;
    pub const SIGNATURE: u8 = 20;
    pub const CUSTOM_VARS: u8 = 21;
    pub const ADVERT_PATH: u8 = 22;
    pub const TUNING_PARAMS: u8 = 23;
    pub const STATS: u8 = 24;
    pub const AUTOADD_CONFIG: u8 = 25;
    pub const ALLOWED_REPEAT_FREQ: u8 = 26;
    pub const CHANNEL_DATA_RECV: u8 = 27;
    pub const DEFAULT_FLOOD_SCOPE: u8 = 28;
    pub const CLI_REPLY: u8 = 29;

    pub const ADVERT: u8 = 0x80;
    pub const PATH_UPDATED: u8 = 0x81;
    pub const SEND_CONFIRMED: u8 = 0x82;
    pub const MSG_WAITING: u8 = 0x83;
    pub const RAW_DATA: u8 = 0x84;
    pub const LOGIN_SUCCESS: u8 = 0x85;
    pub const LOGIN_FAIL: u8 = 0x86;
    pub const STATUS_RESPONSE: u8 = 0x87;
    pub const LOG_RX_DATA: u8 = 0x88;
    pub const TRACE_DATA: u8 = 0x89;
    pub const NEW_ADVERT: u8 = 0x8A;
    pub const TELEMETRY_RESPONSE: u8 = 0x8B;
    pub const BINARY_RESPONSE: u8 = 0x8C;
    pub const PATH_DISCOVERY_RESPONSE: u8 = 0x8D;
    pub const CONTROL_DATA: u8 = 0x8E;
    pub const CONTACT_DELETED: u8 = 0x8F;
    pub const CONTACTS_FULL: u8 = 0x90;

    /// Vrai pour un code de v1.18 (les autres deviennent [`crate::Response::Other`]).
    pub const fn is_known(code: u8) -> bool {
        code <= CLI_REPLY || (code >= ADVERT && code <= CONTACTS_FULL)
    }
}

/// Codes d'erreur de l'appareil (réponse ERR), M:135-140.
pub mod err {
    pub const UNSUPPORTED: u8 = 1;
    pub const NOT_FOUND: u8 = 2;
    pub const TABLE_FULL: u8 = 3;
    pub const BAD_STATE: u8 = 4;
    pub const FILE_IO: u8 = 5;
    pub const ILLEGAL_ARG: u8 = 6;
}

/// Trame refusée par le décodage strict.
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

/// Refus de [`Command::decode_device`] : ce que répond `MyMesh::handleCmdFrame` à ce stade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReject {
    /// Trame vide ou de plus de 176 octets : les porteurs ne la remettent jamais au C++ (§1),
    /// pas de réponse.
    Ignored,
    /// Réponse `ERR code` (le plus souvent 1, UNSUPPORTED_CMD : code inconnu ou précondition de
    /// longueur ratée).
    Err(u8),
}

impl DeviceReject {
    /// Code inconnu ou précondition de longueur ratée (dernier `else`, M:2059-2061).
    pub const UNSUPPORTED: Self = DeviceReject::Err(err::UNSUPPORTED);
    /// Précondition ratée sur la forme de la trame quand le C++ répond ILLEGAL_ARG.
    pub const ILLEGAL_ARG: Self = DeviceReject::Err(err::ILLEGAL_ARG);
}

/// Rien ne doit rester à lire.
pub(crate) fn end(r: &Reader<'_>) -> Result<(), DecodeError> {
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

/// Octets d'un chemin d'après son `path_len` : `(path_len & 63) × ((path_len >> 6) + 1)`.
/// `None` si la taille de hash vaut 4 (réservée) ou si le chemin dépasse 64 octets
/// (`Packet::isValidPathLen` et `Packet::writePath`, `src/Packet.cpp:13-30`).
pub const fn path_bytes(path_len: u8) -> Option<usize> {
    let size = (path_len >> 6) as usize + 1;
    let n = (path_len & 63) as usize * size;
    if size == 4 || n > MAX_PATH {
        None
    } else {
        Some(n)
    }
}

/// Lit `path_len` puis le chemin qu'il annonce ; `Value` si `path_len` est invalide.
pub(crate) fn read_path<'a>(r: &mut Reader<'a>) -> Result<(u8, &'a [u8]), DecodeError> {
    let path_len = r.u8()?;
    let n = path_bytes(path_len).ok_or(DecodeError::Value)?;
    Ok((path_len, r.take(n)?))
}

pub(crate) fn i8_of(b: u8) -> i8 {
    i8::from_le_bytes([b])
}

/// Octets affichés comme une chaîne (caractères échappés, octets invalides en `\xNN`).
pub(crate) struct Text<'a>(pub(crate) &'a [u8]);

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
