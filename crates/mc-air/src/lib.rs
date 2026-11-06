//! MeshCore over-the-air packets (`docs/spec/air.md`): codec, payloads, crypto, hashes, scope
//! codes, advert data. Byte-for-byte compatible with MeshCore v1.18.
//!
//! `no_std`, no allocation, no floats; zero-copy decoders that never panic.
#![no_std]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )
)]

pub mod advert;
pub mod crypto;
pub mod packet;
pub mod payload;

pub use advert::AppData;
pub use crypto::{ChannelSecret, LocalIdentity};
pub use packet::{Header, Packet, PacketBuf, PathLen, PayloadType, Route};
pub use payload::Payload;

/// Decoding, building and crypto errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Packet or field shorter than announced.
    Truncated,
    /// Header version other than 0.
    Version,
    /// Reserved path hash size (4 bytes) or outside 1..3.
    PathHashSize,
    /// Path longer than 64 bytes or more than 63 hops.
    PathTooLong,
    /// Path length inconsistent with `path_len`.
    PathMismatch,
    /// Transport codes present without a route type that carries them, or the reverse.
    Transport,
    /// Payload longer than 184 bytes.
    PayloadTooLong,
    /// Packet longer than 255 bytes.
    FrameTooLong,
    /// Output buffer too small.
    Overflow,
    /// Ciphertext empty or not a multiple of 16.
    CipherLen,
    /// Wrong MAC.
    Mac,
    /// Signature failure.
    Signature,
    /// Text too long or containing a NUL.
    TextTooLong,
    /// Unsupported type or variant.
    Unsupported,
}
