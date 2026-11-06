//! Common parts of the MeshCore companion protocol (`docs/spec/companion.md`): codes, errors,
//! fixed-size fields, paths. The frames themselves live in [`crate::command`] (app → device) and
//! [`crate::response`] (device → app).
//!
//! Zero-copy decoding. **Strict** decoding is canonical: `encode(decode(x)) == x` for every
//! accepted frame, and `encode` only writes frames that `decode` reads back identically. Fixed-size
//! fields are kept as is (tail of a name, reserved bytes), even when the device leaves meaningless
//! bytes there. **Device-side** command decoding ([`crate::Command::decode_device`]) reproduces the
//! C++ tolerances (§10, ADR-0002).

use core::fmt;
use mc_core::{Reader, Truncated};

pub use crate::command::{Command, FloodScopeKey};
pub use crate::response::{
    Contact, DeviceInfo, LoginServer, Response, SelfInfo, Stats, freq_ranges,
};

/// Maximum frame size (the firmware's `MAX_FRAME_SIZE`).
pub const FRAME_MAX: usize = 176;
/// Protocol version announced in DEVICE_QUERY: received messages in V3 format.
pub const APP_VER: u8 = 3;
/// `path_len` of an unknown path, or of a flood.
pub const PATH_UNKNOWN: u8 = 0xFF;
/// Type of a "companion" contact (`ADV_TYPE_CHAT`).
pub const ADV_TYPE_CHAT: u8 = 1;
/// Maximum path size in bytes (`MAX_PATH_SIZE`, `src/MeshCore.h:22`).
pub const MAX_PATH: usize = 64;

/// Command codes (app → device), M:9-73. 44 to 49 and 53 are unassigned (ERR 1).
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

/// Response codes (0..29) and notification codes (0x80..0x90), M:76-134.
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

    /// True for a v1.18 code (the others become [`crate::Response::Other`]).
    pub const fn is_known(code: u8) -> bool {
        code <= CLI_REPLY || (code >= ADVERT && code <= CONTACTS_FULL)
    }
}

/// Device error codes (ERR response), M:135-140.
pub mod err {
    pub const UNSUPPORTED: u8 = 1;
    pub const NOT_FOUND: u8 = 2;
    pub const TABLE_FULL: u8 = 3;
    pub const BAD_STATE: u8 = 4;
    pub const FILE_IO: u8 = 5;
    pub const ILLEGAL_ARG: u8 = 6;
}

/// Frame rejected by strict decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// Shorter than its fixed body, or empty.
    Truncated,
    /// Extra bytes, or wrong announced length.
    Length,
    /// Value outside what the spec describes.
    Value,
    /// Unknown command code (a response with an unknown code, by contrast, is [`Response::Other`]).
    Code,
}

impl From<Truncated> for DecodeError {
    fn from(_: Truncated) -> Self {
        DecodeError::Truncated
    }
}

/// Rejection by [`Command::decode_device`]: what `MyMesh::handleCmdFrame` replies at that point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReject {
    /// Empty frame or longer than 176 bytes: the carriers never hand it to the C++ (§1), no
    /// response.
    Ignored,
    /// `ERR code` response (most often 1, UNSUPPORTED_CMD: unknown code or failed length
    /// precondition).
    Err(u8),
}

impl DeviceReject {
    /// Unknown code or failed length precondition (final `else`, M:2059-2061).
    pub const UNSUPPORTED: Self = DeviceReject::Err(err::UNSUPPORTED);
    /// Failed precondition on the frame's shape where the C++ replies ILLEGAL_ARG.
    pub const ILLEGAL_ARG: Self = DeviceReject::Err(err::ILLEGAL_ARG);
}

/// Nothing must be left to read.
pub(crate) fn end(r: &Reader<'_>) -> Result<(), DecodeError> {
    if r.is_empty() {
        Ok(())
    } else {
        Err(DecodeError::Length)
    }
}

/// A fixed-size name (UTF-8, 0x00-terminated, 0x00-padded): the part before the 0x00.
pub fn fixed_str(b: &[u8]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b.get(..n).unwrap_or(&[])
}

/// Writes `s` into a fixed-size field: truncated to keep a 0x00, padded with 0x00.
pub fn to_fixed<const N: usize>(s: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    for (o, b) in out.iter_mut().zip(s.iter().take(N.saturating_sub(1))) {
        *o = *b;
    }
    out
}

/// Path bytes according to its `path_len`: `(path_len & 63) × ((path_len >> 6) + 1)`.
/// `None` if the hash size is 4 (reserved) or if the path exceeds 64 bytes
/// (`Packet::isValidPathLen` and `Packet::writePath`, `src/Packet.cpp:13-30`).
pub const fn path_bytes(path_len: u8) -> Option<usize> {
    let size = (path_len >> 6) as usize + 1;
    let n = (path_len & 63) as usize * size;
    if size == 4 || n > MAX_PATH {
        None
    } else {
        Some(n)
    }
}

/// Reads `path_len`, then the path it announces; `Value` if `path_len` is invalid.
pub(crate) fn read_path<'a>(r: &mut Reader<'a>) -> Result<(u8, &'a [u8]), DecodeError> {
    let path_len = r.u8()?;
    let n = path_bytes(path_len).ok_or(DecodeError::Value)?;
    Ok((path_len, r.take(n)?))
}

pub(crate) fn i8_of(b: u8) -> i8 {
    i8::from_le_bytes([b])
}

/// Bytes displayed as a string (characters escaped, invalid bytes as `\xNN`).
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
